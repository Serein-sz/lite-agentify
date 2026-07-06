use std::{str::FromStr, sync::Mutex, time::Instant};

use anyhow::Context;
use axum::{
    Router,
    body::{Body, Bytes, to_bytes},
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header::CONTENT_TYPE},
    response::{IntoResponse, Response},
    routing::{any, get},
};
use chrono::Utc;
use futures_util::StreamExt;
use serde_json::Value;
use tracing::{info, warn};
use uuid::Uuid;

use super::{
    config::GatewayConfig,
    domain::{TokenUsage, UsageSource},
    headers::{is_response_header_forwardable, outbound_headers},
    model::{Protocol, Provider},
    pricing::calculate_cost,
    state::GatewayState,
    upstream::{UpstreamRequest, UpstreamResponse},
    usage::{
        UsageObserver, UsageRecord, parse_non_streaming_usage, recorder_from_config,
        warn_record_error,
    },
};

pub async fn build_router(config: GatewayConfig) -> anyhow::Result<Router> {
    let recorder = recorder_from_config(config.usage_database.as_ref()).await?;
    let state = GatewayState::from_config_with_upstream_and_recorder(
        config,
        std::sync::Arc::new(super::upstream::HyperUpstreamClient::new()),
        recorder,
    )?;
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

    let ctx = ProxyContext {
        state: &state,
        request_id: &request_id,
        path: &path,
        query: query.as_deref(),
        method: &method,
        original_headers: &original_headers,
        body: &body,
        started_at,
    };

    let mut last_error: Option<Response> = None;
    let mut unresolved_model_alias = false;

    for provider_id in &route.provider_ids {
        let Some(provider) = state.provider(provider_id) else {
            // from_config guarantees every id resolves; skip defensively.
            continue;
        };

        match ctx.attempt_provider(provider).await {
            ProviderAttempt::Forward(response) => return response,
            ProviderAttempt::Failover(response) => last_error = Some(response),
            ProviderAttempt::AliasMissing => unresolved_model_alias = true,
        }
    }

    last_error.unwrap_or_else(|| {
        if unresolved_model_alias {
            (
                StatusCode::BAD_GATEWAY,
                "no provider could resolve model alias",
            )
                .into_response()
        } else {
            (StatusCode::BAD_GATEWAY, "upstream request failed").into_response()
        }
    })
}

/// The outcome of attempting a single provider in a route's failover chain.
enum ProviderAttempt {
    /// A terminal response to forward to the client; no further providers are tried.
    Forward(Response),
    /// A recoverable failure (transport error or HTTP 5xx); record and try the next provider.
    Failover(Response),
    /// The provider has model aliases but does not define the requested alias; skip it.
    AliasMissing,
}

/// Shared, immutable request context for a single proxied request, passed to
/// each provider attempt so the failover loop body stays small.
struct ProxyContext<'a> {
    state: &'a GatewayState,
    request_id: &'a str,
    path: &'a str,
    query: Option<&'a str>,
    method: &'a Method,
    original_headers: &'a HeaderMap,
    body: &'a Bytes,
    started_at: Instant,
}

impl ProxyContext<'_> {
    async fn attempt_provider(&self, provider: &Provider) -> ProviderAttempt {
        let request_id = self.request_id;
        let path = self.path;

        let provider_body = match body_for_provider(self.body, provider) {
            Ok(Some(body)) => body,
            Ok(None) => {
                warn!(
                    %request_id,
                    %path,
                    provider = %provider.id,
                    "provider does not define requested model alias"
                );
                return ProviderAttempt::AliasMissing;
            }
            Err(error) => {
                warn!(%request_id, %path, provider = %provider.id, error = %error, "failed to resolve model alias");
                return ProviderAttempt::Forward(
                    (StatusCode::BAD_REQUEST, "failed to resolve model alias").into_response(),
                );
            }
        };

        let upstream_uri = match upstream_uri(provider, path, self.query) {
            Ok(uri) => uri,
            Err(error) => {
                warn!(%request_id, %path, provider = %provider.id, error = %error, "failed to build upstream URI");
                return ProviderAttempt::Failover(
                    (StatusCode::BAD_GATEWAY, "failed to build upstream URI").into_response(),
                );
            }
        };

        let headers = match outbound_headers(self.original_headers, provider) {
            Ok(headers) => headers,
            Err(error) => {
                warn!(%request_id, %path, provider = %provider.id, error = %error, "failed to build outbound headers");
                return ProviderAttempt::Failover(
                    (StatusCode::BAD_GATEWAY, "failed to build outbound headers").into_response(),
                );
            }
        };

        let result = self
            .state
            .upstream
            .send(UpstreamRequest {
                method: self.method.clone(),
                uri: upstream_uri,
                headers,
                body: provider_body.body.clone(),
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
                    latency_ms = self.started_at.elapsed().as_millis(),
                    "provider returned server error, trying next provider in chain"
                );
                ProviderAttempt::Failover(
                    (StatusCode::BAD_GATEWAY, "upstream request failed").into_response(),
                )
            }
            Ok(upstream) => ProviderAttempt::Forward(
                self.forward_upstream_response(provider, provider_body, upstream)
                    .await,
            ),
            Err(error) => {
                warn!(
                    %request_id,
                    provider = %provider.id,
                    protocol = %provider.protocol,
                    %path,
                    error = %error,
                    latency_ms = self.started_at.elapsed().as_millis(),
                    "upstream llm request failed, trying next provider in chain"
                );
                ProviderAttempt::Failover(
                    (StatusCode::BAD_GATEWAY, "upstream request failed").into_response(),
                )
            }
        }
    }

    async fn forward_upstream_response(
        &self,
        provider: &Provider,
        provider_body: ProviderBody,
        upstream: UpstreamResponse,
    ) -> Response {
        let request_id = self.request_id;
        let path = self.path;
        let status = upstream.status;
        let response_headers = upstream.headers.clone();
        let mut response = Response::builder().status(status);
        for (name, value) in response_headers.iter() {
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
            latency_ms = self.started_at.elapsed().as_millis(),
            "proxied llm request"
        );

        let metadata = UsageMetadata {
            request_id: request_id.to_owned(),
            provider_id: provider.id.clone(),
            protocol: provider.protocol,
            path: path.to_owned(),
            requested_model: provider_body.requested_model,
            upstream_model: provider_body.upstream_model,
            status: status.as_u16(),
            latency_ms: self.started_at.elapsed().as_millis() as i64,
        };

        if is_streaming_response(&response_headers) {
            let body = usage_observed_stream(self.state.clone(), metadata, upstream.body);
            return response
                .body(body)
                .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response());
        }

        let body_bytes = match to_bytes(upstream.body, usize::MAX).await {
            Ok(body) => body,
            Err(error) => {
                warn!(%request_id, provider = %provider.id, %path, error = %error, "failed to buffer upstream response body for usage recording");
                return (StatusCode::BAD_GATEWAY, "failed to read upstream response")
                    .into_response();
            }
        };

        record_usage(
            self.state,
            metadata,
            UsageSource::ProviderResponse,
            parse_non_streaming_usage(provider.protocol, &body_bytes),
        )
        .await;

        response
            .body(Body::from(body_bytes))
            .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response())
    }
}

#[derive(Clone)]
struct UsageMetadata {
    request_id: String,
    provider_id: String,
    protocol: Protocol,
    path: String,
    requested_model: Option<String>,
    upstream_model: Option<String>,
    status: u16,
    latency_ms: i64,
}

async fn record_usage(
    state: &GatewayState,
    metadata: UsageMetadata,
    source: UsageSource,
    usage: Option<TokenUsage>,
) {
    let usage_source = if usage.is_some() {
        source
    } else {
        UsageSource::Unavailable
    };
    let usage = usage.unwrap_or_default();
    let cost = calculate_cost(
        &state.pricing,
        &metadata.provider_id,
        metadata.upstream_model.as_deref(),
        &usage,
    );
    let (estimated_cost, currency, pricing_source) = cost
        .map(|(cost, currency, pricing_source)| (Some(cost), Some(currency), pricing_source))
        .unwrap_or((None, None, None));

    let record = UsageRecord {
        request_id: metadata.request_id,
        created_at: Utc::now(),
        provider_id: metadata.provider_id,
        protocol: metadata.protocol,
        path: metadata.path,
        requested_model: metadata.requested_model,
        upstream_model: metadata.upstream_model,
        status: metadata.status,
        latency_ms: metadata.latency_ms,
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cached_input_tokens: usage.cached_input_tokens,
        cache_read_tokens: usage.cache_read_tokens,
        cache_write_tokens: usage.cache_write_tokens,
        total_tokens: usage.total_tokens,
        estimated_cost,
        currency,
        usage_source,
        pricing_source,
    };

    if let Err(error) = state.usage_recorder.record(record).await {
        warn_record_error(error);
    }
}

fn usage_observed_stream(state: GatewayState, metadata: UsageMetadata, body: Body) -> Body {
    let observer = std::sync::Arc::new(Mutex::new(UsageObserver::new(metadata.protocol)));
    let stream_observer = observer.clone();
    let stream = body.into_data_stream();

    Body::from_stream(async_stream::stream! {
        futures_util::pin_mut!(stream);
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(bytes) => {
                    if let Ok(mut observer) = stream_observer.lock() {
                        observer.feed(&bytes);
                    }
                    yield Ok::<Bytes, axum::Error>(bytes);
                }
                Err(error) => {
                    yield Err(error);
                    return;
                }
            }
        }

        let usage = observer
            .lock()
            .ok()
            .and_then(|mut observer| observer.finish());
        record_usage(&state, metadata, UsageSource::StreamSummary, usage).await;
    })
}

fn is_streaming_response(headers: &HeaderMap) -> bool {
    headers
        .get(CONTENT_TYPE)
        .and_then(|value: &HeaderValue| value.to_str().ok())
        .is_some_and(|value| value.to_ascii_lowercase().contains("text/event-stream"))
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

struct ProviderBody {
    body: Bytes,
    requested_model: Option<String>,
    upstream_model: Option<String>,
}

fn body_for_provider(body: &Bytes, provider: &Provider) -> anyhow::Result<Option<ProviderBody>> {
    let requested_model = request_model(body);
    if provider.model_aliases.is_empty() || body.is_empty() {
        return Ok(Some(ProviderBody {
            body: body.clone(),
            upstream_model: requested_model.clone(),
            requested_model,
        }));
    }

    let mut value =
        serde_json::from_slice::<Value>(body).context("failed to parse JSON request body")?;
    let Some(model) = value.get("model").and_then(Value::as_str) else {
        return Ok(Some(ProviderBody {
            body: body.clone(),
            upstream_model: requested_model.clone(),
            requested_model,
        }));
    };
    let Some(upstream_model) = provider.model_aliases.get(model) else {
        return Ok(None);
    };

    if let Some(object) = value.as_object_mut() {
        object.insert("model".to_owned(), Value::String(upstream_model.clone()));
    }

    Ok(Some(ProviderBody {
        body: Bytes::from(
            serde_json::to_vec(&value).context("failed to serialize JSON request body")?,
        ),
        requested_model,
        upstream_model: Some(upstream_model.clone()),
    }))
}

fn request_model(body: &[u8]) -> Option<String> {
    if body.is_empty() {
        return None;
    }

    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("model")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
}
