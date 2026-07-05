use std::{str::FromStr, time::Instant};

use anyhow::Context;
use axum::{
    Router,
    body::to_bytes,
    extract::{Request, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{any, get},
};
use tracing::{info, warn};
use uuid::Uuid;

use super::{
    config::GatewayConfig,
    headers::{is_response_header_forwardable, outbound_headers},
    model::Provider,
    state::GatewayState,
    upstream::UpstreamRequest,
};

pub fn build_router(config: GatewayConfig) -> anyhow::Result<Router> {
    let state = GatewayState::from_config(config)?;
    Ok(build_router_with_state(state))
}

pub(super) fn build_router_with_state(state: GatewayState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .fallback(any(proxy))
        .with_state(state)
}

async fn healthz() -> &'static str {
    "ok"
}

async fn proxy(State(state): State<GatewayState>, request: Request) -> Response {
    let started_at = Instant::now();
    let request_id = request_id(request.headers());
    let path = request.uri().path().to_owned();
    let query = request.uri().query().map(str::to_owned);
    let method = request.method().clone();
    let original_headers = request.headers().clone();

    if !state.is_authorized(&original_headers) {
        warn!(%request_id, %path, "rejected unauthenticated gateway request");
        return (
            StatusCode::UNAUTHORIZED,
            "missing or invalid gateway bearer token",
        )
            .into_response();
    }

    let body = match to_bytes(request.into_body(), usize::MAX).await {
        Ok(body) => body,
        Err(error) => {
            warn!(%request_id, %path, error = %error, "failed to read request body");
            return (StatusCode::BAD_REQUEST, "failed to read request body").into_response();
        }
    };

    let Some(route) = state.match_route(&path, &body) else {
        warn!(%request_id, %path, "no gateway route matched");
        return (StatusCode::NOT_FOUND, "no matching gateway route").into_response();
    };

    let mut last_error: Option<Response> = None;

    for provider_id in &route.provider_ids {
        let Some(provider) = state.provider(provider_id) else {
            // from_config guarantees every id resolves; skip defensively.
            continue;
        };

        let upstream_uri = match upstream_uri(provider, &path, query.as_deref()) {
            Ok(uri) => uri,
            Err(error) => {
                warn!(%request_id, %path, provider = %provider.id, error = %error, "failed to build upstream URI");
                last_error =
                    Some((StatusCode::BAD_GATEWAY, "failed to build upstream URI").into_response());
                continue;
            }
        };

        let headers = match outbound_headers(&original_headers, provider) {
            Ok(headers) => headers,
            Err(error) => {
                warn!(%request_id, %path, provider = %provider.id, error = %error, "failed to build outbound headers");
                last_error = Some(
                    (StatusCode::BAD_GATEWAY, "failed to build outbound headers").into_response(),
                );
                continue;
            }
        };

        let result = state
            .upstream
            .send(UpstreamRequest {
                method: method.clone(),
                uri: upstream_uri,
                headers,
                body: body.clone(),
            })
            .await;

        match result {
            Ok(upstream) if upstream.status.is_server_error() => {
                warn!(
                    %request_id,
                    provider = %provider.id,
                    protocol = %provider.protocol,
                    %path,
                    status = upstream.status.as_u16(),
                    latency_ms = started_at.elapsed().as_millis(),
                    "provider returned server error, trying next provider in chain"
                );
                last_error = Some((StatusCode::BAD_GATEWAY, "upstream request failed").into_response());
                continue;
            }
            Ok(upstream) => {
                let status = upstream.status;
                let mut response = Response::builder().status(status);
                for (name, value) in upstream.headers.iter() {
                    if is_response_header_forwardable(name) {
                        response = response.header(name, value);
                    }
                }

                info!(
                    %request_id,
                    provider = %provider.id,
                    protocol = %provider.protocol,
                    %path,
                    status = status.as_u16(),
                    latency_ms = started_at.elapsed().as_millis(),
                    "proxied llm request"
                );

                return response
                    .body(upstream.body)
                    .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response());
            }
            Err(error) => {
                warn!(
                    %request_id,
                    provider = %provider.id,
                    protocol = %provider.protocol,
                    %path,
                    error = %error,
                    latency_ms = started_at.elapsed().as_millis(),
                    "upstream llm request failed, trying next provider in chain"
                );
                last_error = Some((StatusCode::BAD_GATEWAY, "upstream request failed").into_response());
                continue;
            }
        }
    }

    last_error.unwrap_or_else(|| (StatusCode::BAD_GATEWAY, "upstream request failed").into_response())
}

fn request_id(headers: &HeaderMap) -> String {
    headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
        .unwrap_or_else(|| Uuid::new_v4().to_string())
}

fn upstream_uri(provider: &Provider, path: &str, query: Option<&str>) -> anyhow::Result<Uri> {
    let mut uri = format!("{}{}", provider.base_url, path);
    if let Some(query) = query {
        uri.push('?');
        uri.push_str(query);
    }
    Uri::from_str(&uri).context("invalid upstream URI")
}
