use async_graphql::http::MultipartOptions;
use async_graphql::ParseRequestError;
use salvo::http::Method;
use salvo::{http, Request};
use std::marker::PhantomData;

pub struct GraphQLRequest(async_graphql::Request, PhantomData<ParseRequestError>);

impl GraphQLRequest {
    pub(crate) async fn from_request(req: &mut Request) -> Result<Self, ParseRequestError> {
        Ok(GraphQLRequest(
            GraphQLBatchRequest::from_request(req)
                .await?
                .0
                .into_single()?,
            PhantomData,
        ))
    }

    #[must_use]
    pub fn into_inner(self) -> async_graphql::Request {
        self.0
    }
}

/// Extractor for GraphQL batch request.
pub struct GraphQLBatchRequest(async_graphql::BatchRequest, PhantomData<ParseRequestError>);

impl GraphQLBatchRequest {
    /// Unwraps the value to `async_graphql::BatchRequest`.
    #[must_use]
    pub fn into_inner(self) -> async_graphql::BatchRequest {
        self.0
    }
}

impl GraphQLBatchRequest {
    pub(crate) async fn from_request(req: &mut Request) -> Result<Self, ParseRequestError> {
        if req.method() == Method::GET {
            let uri = req.uri();
            let res = async_graphql::http::parse_query_string(uri.query().unwrap_or_default())
                .map_err(|err| {
                    ParseRequestError::Io(std::io::Error::other(format!(
                        "failed to parse graphql request from uri query: {}",
                        err
                    )))
                });
            Ok(Self(async_graphql::BatchRequest::Single(res?), PhantomData))
        } else {
            let content_type = req
                .headers()
                .get(http::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string);
            let payload = req.payload().await;
            match payload {
                Ok(payload) => {
                    let payload = payload.iter().as_slice();
                    Ok(Self(
                        async_graphql::http::receive_batch_body(
                            content_type,
                            payload,
                            MultipartOptions::default(),
                        )
                        .await?,
                        PhantomData,
                    ))
                }
                Err(e) => Err(ParseRequestError::InvalidRequest(e.into())),
            }
        }
    }
}
