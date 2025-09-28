use async_graphql::http::{
    default_on_connection_init, default_on_ping, DefaultOnConnInitType, DefaultOnPingType,
    WebSocketProtocols, WsMessage,
};
use async_graphql::{Data, Executor};
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{future, Sink, SinkExt, Stream, StreamExt};
use salvo::__private::tracing;
use salvo::http::{header, HeaderValue, StatusError};
use salvo::websocket::{Message, WebSocketUpgrade};
use salvo::{async_trait, http, Depot, FlowCtrl, Handler, Request, Response};
use std::str::FromStr;
use std::time::Duration;

pub struct GraphQLSubscription<E> {
    executor: E,
}

impl<E> Clone for GraphQLSubscription<E>
where
    E: Executor,
{
    fn clone(&self) -> Self {
        Self {
            executor: self.executor.clone(),
        }
    }
}

impl<E> GraphQLSubscription<E>
where
    E: Executor,
{
    /// Create a GraphQL subscription service.
    pub fn new(executor: E) -> Self {
        Self { executor }
    }
}

impl<E: Executor> GraphQLSubscription<E> {
    async fn execute(
        &self,
        req: &mut Request,
        _depot: &mut Depot,
        res: &mut Response,
        _ctrl: &mut FlowCtrl,
    ) -> Result<(), salvo::Error> {
        let protocol = GraphQLProtocol::from_request(req).await?;
        let sec_websocket_protocol = HeaderValue::from_static(protocol.0.sec_websocket_protocol());
        let executor = self.executor.clone();
        WebSocketUpgrade::new()
            .upgrade(req, res, move |ws| {
                GraphQLWebSocket::new(ws, executor, protocol).serve()
            })
            .await?;
        res.headers_mut()
            .insert(header::SEC_WEBSOCKET_PROTOCOL, sec_websocket_protocol);
        Ok(())
    }
}

#[async_trait]
impl<E: Executor> Handler for GraphQLSubscription<E> {
    async fn handle(
        &self,
        req: &mut Request,
        depot: &mut Depot,
        res: &mut Response,
        ctrl: &mut FlowCtrl,
    ) {
        salvo::Writer::write(self.execute(req, depot, res, ctrl).await, req, depot, res).await
    }
}

/// A GraphQL protocol extractor.
///
/// It extract GraphQL protocol from `SEC_WEBSOCKET_PROTOCOL` header.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct GraphQLProtocol(pub WebSocketProtocols);

impl GraphQLProtocol {
    async fn from_request(req: &mut Request) -> Result<Self, salvo::Error> {
        req.headers()
            .get(http::header::SEC_WEBSOCKET_PROTOCOL)
            .and_then(|value| value.to_str().ok())
            .and_then(|protocols| {
                protocols
                    .split(',')
                    .find_map(|p| WebSocketProtocols::from_str(p.trim()).ok())
            })
            .map(Self)
            .ok_or_else(|| salvo::Error::HttpStatus(StatusError::bad_request()))
    }
}

/// A Websocket connection for GraphQL subscription.
pub struct GraphQLWebSocket<Sink, Stream, E, OnConnInit, OnPing> {
    sink: Sink,
    stream: Stream,
    executor: E,
    data: Data,
    on_connection_init: OnConnInit,
    on_ping: OnPing,
    protocol: GraphQLProtocol,
    keepalive_timeout: Option<Duration>,
}

impl<S, E>
    GraphQLWebSocket<
        SplitSink<S, Message>,
        SplitStream<S>,
        E,
        DefaultOnConnInitType,
        DefaultOnPingType,
    >
where
    S: Stream<Item = Result<Message, salvo::Error>> + Sink<Message>,
    E: Executor,
{
    /// Create a [`GraphQLWebSocket`] object.
    pub fn new(stream: S, executor: E, protocol: GraphQLProtocol) -> Self {
        let (sink, stream) = stream.split();
        GraphQLWebSocket::new_with_pair(sink, stream, executor, protocol)
    }
}

impl<Sink, Stream, E> GraphQLWebSocket<Sink, Stream, E, DefaultOnConnInitType, DefaultOnPingType>
where
    Sink: futures_util::sink::Sink<Message>,
    Stream: futures_util::stream::Stream<Item = Result<Message, salvo::Error>>,
    E: Executor,
{
    /// Create a [`GraphQLWebSocket`] object with sink and stream objects.
    pub fn new_with_pair(
        sink: Sink,
        stream: Stream,
        executor: E,
        protocol: GraphQLProtocol,
    ) -> Self {
        GraphQLWebSocket {
            sink,
            stream,
            executor,
            data: Data::default(),
            on_connection_init: default_on_connection_init,
            on_ping: default_on_ping,
            protocol,
            keepalive_timeout: None,
        }
    }
}

impl<Sink, Stream, E, OnConnInit, OnConnInitFut, OnPing, OnPingFut>
    GraphQLWebSocket<Sink, Stream, E, OnConnInit, OnPing>
where
    Sink: futures_util::sink::Sink<Message>,
    Stream: futures_util::stream::Stream<Item = Result<Message, salvo::Error>>,
    E: Executor,
    OnConnInit: FnOnce(serde_json::Value) -> OnConnInitFut + Send + 'static,
    OnConnInitFut: Future<Output = async_graphql::Result<Data>> + Send + 'static,
    OnPing: FnOnce(Option<&Data>, Option<serde_json::Value>) -> OnPingFut + Clone + Send + 'static,
    OnPingFut: Future<Output = async_graphql::Result<Option<serde_json::Value>>> + Send + 'static,
{
    #[must_use]
    pub fn with_data(self, data: Data) -> Self {
        Self { data, ..self }
    }

    #[must_use]
    pub fn on_connection_init<F, R>(
        self,
        callback: F,
    ) -> GraphQLWebSocket<Sink, Stream, E, F, OnPing>
    where
        F: FnOnce(serde_json::Value) -> R + Send + 'static,
        R: Future<Output = async_graphql::Result<Data>> + Send + 'static,
    {
        GraphQLWebSocket {
            sink: self.sink,
            stream: self.stream,
            executor: self.executor,
            data: self.data,
            on_connection_init: callback,
            on_ping: self.on_ping,
            protocol: self.protocol,
            keepalive_timeout: self.keepalive_timeout,
        }
    }

    #[must_use]
    pub fn on_ping<F, R>(self, callback: F) -> GraphQLWebSocket<Sink, Stream, E, OnConnInit, F>
    where
        F: FnOnce(Option<&Data>, Option<serde_json::Value>) -> R + Clone + Send + 'static,
        R: Future<Output = async_graphql::Result<Option<serde_json::Value>>> + Send + 'static,
    {
        GraphQLWebSocket {
            sink: self.sink,
            stream: self.stream,
            executor: self.executor,
            data: self.data,
            on_connection_init: self.on_connection_init,
            on_ping: callback,
            protocol: self.protocol,
            keepalive_timeout: self.keepalive_timeout,
        }
    }

    /// Sets a timeout for receiving an acknowledgement of the keep-alive ping.
    ///
    /// If the ping is not acknowledged within the timeout, the connection will
    /// be closed.
    ///
    /// NOTE: Only used for the `graphql-ws` protocol.
    #[must_use]
    pub fn keepalive_timeout(self, timeout: impl Into<Option<Duration>>) -> Self {
        Self {
            keepalive_timeout: timeout.into(),
            ..self
        }
    }

    /// Processing subscription requests.
    pub async fn serve(self) {
        let input = self
            .stream
            .take_while(|res| future::ready(res.is_ok()))
            .filter_map(|res| match res {
                Ok(msg) => future::ready(Some(msg)),
                Err(err) => {
                    tracing::error!("{}", err);
                    future::ready(None)
                }
            })
            .filter_map(|msg| {
                if msg.is_text() || msg.is_binary() {
                    future::ready(Some(msg))
                } else {
                    future::ready(None)
                }
            })
            .map(|msg| <Message as Into<Vec<u8>>>::into(msg));

        let stream =
            async_graphql::http::WebSocket::new(self.executor.clone(), input, self.protocol.0)
                .connection_data(self.data)
                .on_connection_init(self.on_connection_init)
                .on_ping(self.on_ping.clone())
                .keepalive_timeout(self.keepalive_timeout)
                .map(|msg| match msg {
                    WsMessage::Text(text) => Message::text(text),
                    WsMessage::Close(code, status) => Message::close_with(code, status),
                });

        let sink = self.sink;
        futures_util::pin_mut!(stream, sink);

        // loop {
        //     if let Some(item) = stream.next().await {
        //         if sink.send(item).await.is_err() {
        //             tracing::error!("Error sending message to sink.");
        //             break;
        //         }
        //     }
        // }

        while let Some(item) = stream.next().await {
            if sink.send(item).await.is_err() {
                tracing::error!("Error sending message to sink.");
                break;
            }
        }
    }
}
