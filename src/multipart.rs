use async_graphql::{async_stream, Response};
use bytes::{BufMut, Bytes, BytesMut};
use futures_timer::Delay;
use futures_util::{stream::BoxStream, FutureExt, Stream, StreamExt};
use salvo::http::body::BytesFrame;
use salvo::BoxedError;
use std::time::Duration;

static PART_HEADER: Bytes = Bytes::from_static(b"--graphql\r\nContent-Type: application/json\r\n\r\n");
static EOF: Bytes = Bytes::from_static(b"--graphql--\r\n");
static CRLF: Bytes = Bytes::from_static(b"\r\n");
static HEARTBEAT: Bytes = Bytes::from_static(b"{}\r\n");

/// Create a stream for `multipart/mixed` responses.
///
/// Reference: <https://www.apollographql.com/docs/router/executing-operations/subscription-multipart-protocol/>
pub fn create_multipart_mixed_stream<'a>(
    input: impl Stream<Item = Response> + Send + Unpin + 'a,
    heartbeat_interval: Duration,
) -> BoxStream<'a, Result<BytesFrame, BoxedError>> {
    let mut input = input.fuse();
    let mut heartbeat_timer = Delay::new(heartbeat_interval).fuse();

    async_stream::stream! {
        loop {
            futures_util::select! {
                item = input.next() => {
                    match item {
                        Some(resp) => {
                            let data = BytesMut::new();
                            let mut writer = data.writer();
                            if serde_json::to_writer(&mut writer, &resp).is_err() {
                                continue;
                            }


                            yield Ok(BytesFrame::data(PART_HEADER.clone()));
                            yield Ok(BytesFrame::data(writer.into_inner().freeze()));
                            yield Ok(BytesFrame::data(CRLF.clone()));
                        }
                        None => break,
                    }
                }
                _ = heartbeat_timer => {
                    heartbeat_timer = Delay::new(heartbeat_interval).fuse();
                    yield Ok(BytesFrame::data(PART_HEADER.clone()));
                    yield Ok(BytesFrame::data(HEARTBEAT.clone()));
                }
            }
        }

        yield Ok(BytesFrame::data(EOF.clone()));
    }
    .boxed()
}
