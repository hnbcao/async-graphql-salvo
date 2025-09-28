mod multipart;
mod request;
#[cfg(feature = "websocket")]
pub mod subscription;

use crate::multipart::create_multipart_mixed_stream;
use crate::request::{GraphQLBatchRequest, GraphQLRequest};
use async_graphql::http::is_accept_multipart_mixed;
use async_graphql::{Executor, ParseRequestError};
use salvo::http::{HeaderValue, ResBody, StatusError};
use salvo::hyper::http;
use salvo::{async_trait, Depot, FlowCtrl, Handler, Request, Response};
use std::time::Duration;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum GraphQLError {
    #[error("{0}")]
    GraphQLRejection(#[from] ParseRequestError),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Serde(#[from] serde_json::Error),
}

/// A GraphQL service.
#[derive(Clone)]
pub struct GraphQL<E> {
    executor: E,
}

impl<E> GraphQL<E>
where
    E: Executor,
{
    pub fn new(executor: E) -> Self {
        Self { executor }
    }
}

#[async_trait]
impl<E: Executor> Handler for GraphQL<E> {
    async fn handle(
        &self,
        req: &mut Request,
        depot: &mut Depot,
        res: &mut Response,
        _ctrl: &mut FlowCtrl,
    ) {
        let is_accept_multipart_mixed = req
            .headers()
            .get("accept")
            .and_then(|value| value.to_str().ok())
            .map(is_accept_multipart_mixed)
            .unwrap_or_default();

        let result = self
            .execute(req, res, is_accept_multipart_mixed)
            .await
            .map_err(|e| match e {
                GraphQLError::GraphQLRejection(e) => match e {
                    ParseRequestError::PayloadTooLarge => {
                        salvo::Error::HttpStatus(StatusError::payload_too_large())
                    }
                    other => salvo::Error::other(other),
                },
                GraphQLError::Io(e) => salvo::Error::Io(e),
                GraphQLError::Serde(e) => salvo::Error::other(e),
            });
        salvo::Writer::write(result, req, depot, res).await
    }
}

impl<E: Executor> GraphQL<E> {
    async fn execute(
        &self,
        req: &mut Request,
        res: &mut Response,
        is_accept_multipart_mixed: bool,
    ) -> Result<(), GraphQLError> {
        if is_accept_multipart_mixed {
            let req = GraphQLRequest::from_request(req).await?;
            let stream = self.executor.execute_stream(req.into_inner(), None);
            let box_stream = create_multipart_mixed_stream(stream, Duration::from_secs(30));
            res.headers_mut().insert(
                http::header::CONTENT_TYPE,
                HeaderValue::from_static("multipart/mixed; boundary=graphql"),
            );
            res.body(ResBody::stream(box_stream));
        } else {
            let req = GraphQLBatchRequest::from_request(req).await?;
            let graphql_response = self.executor.execute_batch(req.into_inner()).await;
            let body = serde_json::to_string(&graphql_response)?;
            res.body(body);
            res.headers_mut().insert(
                http::header::CONTENT_TYPE,
                HeaderValue::from_static("application/graphql-response+json"),
            );
            if graphql_response.is_ok() {
                if let Some(cache_control) = graphql_response.cache_control().value() {
                    if let Ok(value) = HeaderValue::from_str(&cache_control) {
                        res.headers_mut().insert(http::header::CACHE_CONTROL, value);
                    }
                }
            }

            res.headers_mut().extend(graphql_response.http_headers());
        }
        Ok(())
    }
}
