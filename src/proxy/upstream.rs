use std::{future::Future, pin::Pin};

use anyhow::Context;
use axum::{
    body::{Body, Bytes},
    http::{HeaderMap, Method, StatusCode, Uri},
};
use http_body_util::{BodyExt, Full};
use hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};
use hyper_util::{
    client::legacy::{Client, connect::HttpConnector},
    rt::TokioExecutor,
};

#[derive(Clone)]
pub(crate) struct UpstreamRequest {
    pub method: Method,
    pub uri: Uri,
    pub headers: HeaderMap,
    pub body: Bytes,
}

pub(crate) struct UpstreamResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Body,
}

pub(crate) type UpstreamFuture =
    Pin<Box<dyn Future<Output = anyhow::Result<UpstreamResponse>> + Send>>;

pub(crate) trait UpstreamClient: Send + Sync {
    fn send(&self, request: UpstreamRequest) -> UpstreamFuture;
}

#[derive(Clone)]
pub(crate) struct HyperUpstreamClient {
    client: Client<HttpsConnector<HttpConnector>, Full<Bytes>>,
}

impl HyperUpstreamClient {
    pub fn new() -> Self {
        let connector = HttpsConnectorBuilder::new()
            .with_webpki_roots()
            .https_or_http()
            .enable_http1()
            .enable_http2()
            .build();
        Self {
            client: Client::builder(TokioExecutor::new()).build(connector),
        }
    }
}

impl UpstreamClient for HyperUpstreamClient {
    fn send(&self, request: UpstreamRequest) -> UpstreamFuture {
        let client = self.client.clone();
        Box::pin(async move {
            let mut builder = hyper::Request::builder()
                .method(request.method)
                .uri(request.uri);

            for (name, value) in request.headers.iter() {
                builder = builder.header(name, value);
            }

            let response = client
                .request(builder.body(Full::new(request.body))?)
                .await
                .context("upstream request failed")?;
            let status = response.status();
            let headers = response.headers().clone();
            let stream = response.into_body().into_data_stream();

            Ok(UpstreamResponse {
                status,
                headers,
                body: Body::from_stream(stream),
            })
        })
    }
}
