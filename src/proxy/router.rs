use std::{
    path::PathBuf,
    str::FromStr,
    sync::Mutex,
    time::{Duration, Instant},
};

use anyhow::{Context, bail};
use axum::{
    Router,
    body::{Body, Bytes, to_bytes},
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header::CONTENT_TYPE},
    response::{IntoResponse, Response},
    routing::{any, get, post},
};
use chrono::Utc;
use futures_util::StreamExt;
use serde_json::Value;
use tracing::{error, info, warn};
use uuid::Uuid;

use super::{
    headers::{is_response_header_forwardable, outbound_headers},
    upstream::{UpstreamRequest, UpstreamResponse},
};
use crate::{
    config::GatewayConfig,
    domain::TokenUsage,
    model::{Protocol, Provider},
    pricing::calculate_cost,
    reload::SharedGatewayState,
    state::GatewayState,
    usage::{
        UsageObserver, UsageRecord, UsageSource, parse_non_streaming_usage, recorder_from_config,
        warn_record_error,
    },
};

pub async fn build_router(
    config: GatewayConfig,
    config_path: PathBuf,
) -> anyhow::Result<(Router, SharedGatewayState)> {
    let recorder = recorder_from_config(config.usage_database.as_ref()).await?;
    let state = GatewayState::from_config_with_upstream_and_recorder(
        config.clone(),
        std::sync::Arc::new(super::upstream::HyperUpstreamClient::new()),
        recorder,
    )?;
    let shared = SharedGatewayState::new(state, &config, config_path);
    Ok((build_router_with_shared(shared.clone()), shared))
}

#[cfg(test)]
pub(crate) fn build_router_with_state(state: GatewayState) -> Router {
    build_router_with_shared(SharedGatewayState::without_reload(state))
}

pub(crate) fn build_router_with_shared(shared: SharedGatewayState) -> Router {
    // The admin console owns the /admin prefix (404 stub when disabled), so
    // nothing under it can ever fall through to the upstream proxy.
    let admin = crate::admin::admin_router(&shared);
    Router::new()
        .route("/healthz", get(healthz))
        .route("/reload", post(reload_endpoint))
        .nest_service("/admin", admin)
        .fallback(any(proxy))
        .with_state(shared)
}

async fn healthz() -> &'static str {
    "ok"
}

async fn reload_endpoint(State(shared): State<SharedGatewayState>, headers: HeaderMap) -> Response {
    if !shared.load().is_authorized(&headers) {
        warn!("rejected unauthenticated gateway reload request");
        return (
            StatusCode::UNAUTHORIZED,
            "missing or invalid gateway bearer token",
        )
            .into_response();
    }

    match crate::reload::reload(&shared) {
        Ok(()) => (StatusCode::OK, "gateway configuration reloaded").into_response(),
        Err(reload_error) => {
            error!(
                error = format!("{reload_error:#}"),
                "config reload via endpoint failed; keeping previous configuration"
            );
            // Top-level message only: full chains can quote config file
            // contents (TOML snippets), which may contain secrets.
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("configuration reload failed: {reload_error}"),
            )
                .into_response()
        }
    }
}

async fn proxy(State(shared): State<SharedGatewayState>, request: Request) -> Response {
    // One snapshot per request: a concurrent reload swaps the shared pointer
    // but never changes the state this request already resolved.
    let state = shared.load();
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

    let payload = RequestPayload::parse(&body);

    let Some(route) = state.match_route(&path, payload.model.as_deref()) else {
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
        payload: &payload,
        started_at,
    };

    let mut last_error: Option<Response> = None;
    let mut unresolved_model_alias = false;

    let provider_count = route.provider_ids.len();
    for (index, provider_id) in route.provider_ids.iter().enumerate() {
        let Some(provider) = state.provider(provider_id) else {
            // from_config guarantees every id resolves; skip defensively.
            continue;
        };

        // The last provider in the chain has nowhere to fail over to, so a
        // provider that exhausts its rate-limit retries forwards its real
        // response (e.g. 429) to the client instead of a synthesized 502.
        let is_last = index + 1 == provider_count;
        match ctx.attempt_provider(provider, is_last).await {
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
    payload: &'a RequestPayload,
    started_at: Instant,
}

impl ProxyContext<'_> {
    /// Attempts one provider, retrying it in place on a configured rate-limit
    /// status (default 429/529) with backoff before giving up. `is_last`
    /// forwards an exhausted rate-limit response to the client rather than
    /// failing over to a non-existent next provider.
    async fn attempt_provider(&self, provider: &Provider, is_last: bool) -> ProviderAttempt {
        let request_id = self.request_id;
        let path = self.path;

        let provider_body = match body_for_provider(self.body, self.payload, provider) {
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

        let retry = &self.state.retry_policy;
        // 0-based retry index; attempt 0 is the initial try. Bounded by
        // `max_attempts` (validated >= 1 at startup).
        for attempt in 0..retry.max_attempts {
            let result = self
                .state
                .upstream
                .send(UpstreamRequest {
                    method: self.method.clone(),
                    uri: upstream_uri.clone(),
                    headers: headers.clone(),
                    body: provider_body.body.clone(),
                })
                .await;

            let upstream = match result {
                Ok(upstream) => upstream,
                Err(error) => {
                    // Transport errors fail over immediately, never retried
                    // against the same provider.
                    warn!(
                        %request_id,
                        provider = %provider.id,
                        protocol = %provider.protocol,
                        %path,
                        error = %error,
                        latency_ms = self.started_at.elapsed().as_millis(),
                        "upstream llm request failed, trying next provider in chain"
                    );
                    return ProviderAttempt::Failover(
                        (StatusCode::BAD_GATEWAY, "upstream request failed").into_response(),
                    );
                }
            };

            let status = upstream.status;
            let is_retryable = retry.is_retryable(status.as_u16());

            // 5xx fails over immediately (no same-provider retry), as before —
            // unless the status is explicitly configured as retryable (e.g. the
            // 529 "overloaded" code), which takes the backoff path below.
            if status.is_server_error() && !is_retryable {
                warn!(
                    %request_id,
                    provider = %provider.id,
                    protocol = %provider.protocol,
                    %path,
                    status = status.as_u16(),
                    latency_ms = self.started_at.elapsed().as_millis(),
                    "provider returned server error, trying next provider in chain"
                );
                return ProviderAttempt::Failover(
                    (StatusCode::BAD_GATEWAY, "upstream request failed").into_response(),
                );
            }

            // A configured rate-limit status is retried against the same
            // provider with backoff until attempts are exhausted.
            if is_retryable {
                let is_final_attempt = attempt + 1 >= retry.max_attempts;
                if is_final_attempt {
                    if is_last {
                        // Nowhere left to fail over; return the real response.
                        info!(
                            %request_id,
                            provider = %provider.id,
                            %path,
                            status = status.as_u16(),
                            attempts = attempt + 1,
                            "rate-limit retries exhausted on last provider, forwarding response to client"
                        );
                        return ProviderAttempt::Forward(
                            self.forward_upstream_response(provider, provider_body, upstream)
                                .await,
                        );
                    }
                    warn!(
                        %request_id,
                        provider = %provider.id,
                        %path,
                        status = status.as_u16(),
                        attempts = attempt + 1,
                        "rate-limit retries exhausted, trying next provider in chain"
                    );
                    return ProviderAttempt::Failover(
                        (StatusCode::BAD_GATEWAY, "upstream request failed").into_response(),
                    );
                }

                let delay = self.backoff_delay(retry, attempt, &upstream.headers);
                warn!(
                    %request_id,
                    provider = %provider.id,
                    %path,
                    status = status.as_u16(),
                    attempt = attempt + 1,
                    delay_ms = delay.as_millis(),
                    "provider rate-limited, backing off before retrying same provider"
                );
                // Drop the rate-limit response body before waiting.
                drop(upstream);
                tokio::time::sleep(delay).await;
                continue;
            }

            // 2xx / 3xx / non-retryable 4xx: forward to the client.
            return ProviderAttempt::Forward(
                self.forward_upstream_response(provider, provider_body, upstream)
                    .await,
            );
        }

        // Unreachable: the loop returns on every path when max_attempts >= 1,
        // which startup validation guarantees. Fail over defensively.
        ProviderAttempt::Failover(
            (StatusCode::BAD_GATEWAY, "upstream request failed").into_response(),
        )
    }

    /// The wait before the next same-provider retry: a capped `Retry-After`
    /// header when present, otherwise full-jitter exponential backoff.
    fn backoff_delay(
        &self,
        retry: &crate::model::RetryPolicy,
        attempt: u32,
        headers: &HeaderMap,
    ) -> Duration {
        if let Some(retry_after_ms) = parse_retry_after_ms(headers) {
            return Duration::from_millis(retry.cap_delay_ms(retry_after_ms));
        }
        Duration::from_millis(retry.backoff_ms(attempt, jitter_fraction()))
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
    let mut usage = usage.unwrap_or_default();
    usage.ensure_total();
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

/// The request body parsed exactly once per request, reused for route matching
/// and every provider's model-alias resolution.
struct RequestPayload {
    /// The parsed JSON value when the body is non-empty and valid JSON.
    json: Option<Value>,
    /// The top-level string `model`, when present.
    model: Option<String>,
}

impl RequestPayload {
    fn parse(body: &Bytes) -> Self {
        if body.is_empty() {
            return Self {
                json: None,
                model: None,
            };
        }
        match serde_json::from_slice::<Value>(body) {
            Ok(value) => {
                let model = value
                    .get("model")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                Self {
                    json: Some(value),
                    model,
                }
            }
            // Non-empty but invalid JSON: no model, no parsed value. The alias
            // branch of body_for_provider treats a missing value as a parse error.
            Err(_) => Self {
                json: None,
                model: None,
            },
        }
    }
}

struct ProviderBody {
    body: Bytes,
    requested_model: Option<String>,
    upstream_model: Option<String>,
}

fn body_for_provider(
    body: &Bytes,
    payload: &RequestPayload,
    provider: &Provider,
) -> anyhow::Result<Option<ProviderBody>> {
    let requested_model = payload.model.clone();
    if provider.model_aliases.is_empty() || body.is_empty() {
        return Ok(Some(ProviderBody {
            body: body.clone(),
            upstream_model: requested_model.clone(),
            requested_model,
        }));
    }

    // Provider has aliases and the body is non-empty: a non-empty body that did
    // not parse as JSON is a request error, matching the previous behavior.
    let Some(value) = payload.json.as_ref() else {
        bail!("failed to parse JSON request body");
    };
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

    let mut value = value.clone();
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

/// Parses a `Retry-After` header into milliseconds. Handles both the
/// delta-seconds form (`Retry-After: 120`) and the HTTP-date form
/// (`Retry-After: Wed, 21 Oct 2015 07:28:00 GMT`). A malformed or past value
/// yields `None`, so the caller falls back to computed backoff.
fn parse_retry_after_ms(headers: &HeaderMap) -> Option<u64> {
    let value = headers.get("retry-after")?.to_str().ok()?.trim();

    if let Ok(seconds) = value.parse::<u64>() {
        return Some(seconds.saturating_mul(1000));
    }

    let target = chrono::DateTime::parse_from_rfc2822(value).ok()?;
    let delta = target.with_timezone(&Utc) - Utc::now();
    delta
        .num_milliseconds()
        .try_into()
        .ok()
        .filter(|&ms: &u64| ms > 0)
}

/// A uniform random fraction in `[0, 1)` for full-jitter backoff, drawn from
/// UUID v4 random bytes to avoid a dedicated RNG dependency.
fn jitter_fraction() -> f64 {
    let bytes = Uuid::new_v4().into_bytes();
    let raw = u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]);
    // Map the 53 significant mantissa bits into [0, 1).
    (raw >> 11) as f64 / (1u64 << 53) as f64
}
