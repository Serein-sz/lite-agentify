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
    model::{Protocol, Provider, protocol_of},
    pricing::calculate_cost,
    reload::SharedGatewayState,
    state::{GatewayState, Resolution},
    usage::{
        UsageObserver, UsageRecord, UsageSource, parse_non_streaming_usage, recorder_from_db,
        warn_record_error,
    },
};

pub async fn build_router(
    config: GatewayConfig,
    config_path: PathBuf,
) -> anyhow::Result<(Router, SharedGatewayState)> {
    // The primary database is mandatory: accounts, API keys, usage records,
    // providers, pricing, and the model catalog all live in PostgreSQL.
    // Startup fails fast when it is unreachable.
    let database_config = config.required_database()?;
    let db = crate::db::connect(database_config).await?;
    crate::db::migrate(&db).await?;
    crate::account::bootstrap_accounts(&db, &config).await?;

    let account_store = crate::account::SeaOrmAccountStore::new(db.clone());
    let api_keys = crate::account::AccountStore::active_key_map(&account_store).await?;

    // One-time file → database imports: providers and pricing (change 2), then
    // routes + aliases → model catalog (change 3). Afterwards the database is
    // the source of truth for all of them.
    let catalog_store = crate::catalog::SeaOrmCatalogStore::new(db.clone());
    crate::catalog::import_config_once(&catalog_store, &config).await?;
    crate::catalog::migrate_routes_once(&catalog_store, &config).await?;
    let catalog = crate::catalog::CatalogStore::snapshot(&catalog_store).await?;

    let recorder = recorder_from_db(db.clone());

    // Redis is the optional hot-state backend: spend counters, admin
    // sessions, login lockout, and the reserved config_changed channel all
    // move there when `[redis]` is configured; otherwise everything stays in
    // process memory.
    let redis = match &config.redis {
        Some(redis_config) => {
            let client = redis::Client::open(redis_config.url.as_str())
                .context("invalid redis url in [redis] config")?;
            let connection = redis::aio::ConnectionManager::new(client.clone())
                .await
                .context("failed to connect redis (the [redis] section is configured)")?;
            info!("redis hot-state backend connected (spend counters, sessions, lockout)");
            Some((client, connection))
        }
        None => None,
    };

    // Spend counters: seeded from Postgres truth before serving; a background
    // loop re-reconciles every interval.
    let quota_store = std::sync::Arc::new(crate::quota::SeaOrmQuotaStore::new(db.clone()));
    let spend_counter: std::sync::Arc<dyn crate::quota::SpendCounter> = match &redis {
        Some((_, connection)) => {
            std::sync::Arc::new(crate::quota::RedisCounter::new(connection.clone()))
        }
        None => std::sync::Arc::new(crate::quota::MemoryCounter::default()),
    };
    crate::quota::reconcile_counters(quota_store.as_ref(), spend_counter.as_ref()).await?;
    crate::quota::spawn_reconciliation(quota_store.clone(), spend_counter.clone());
    let granted = crate::quota::QuotaStore::grant_sums(quota_store.as_ref()).await?;

    // Admin sessions survive restarts in Redis mode; reads fail closed
    // during an outage. The notifier/subscriber pair services the reserved
    // config_changed channel (single-instance no-op).
    let sessions: std::sync::Arc<dyn crate::admin::SessionStore> = match &redis {
        Some((_, connection)) => {
            std::sync::Arc::new(crate::admin::RedisSessionStore::new(connection.clone()))
        }
        None => std::sync::Arc::new(crate::admin::MemorySessionStore::default()),
    };
    let notifier = redis
        .as_ref()
        .map(|(_, connection)| crate::pubsub::ConfigNotifier::new(connection.clone()));
    if let Some((client, _)) = &redis {
        crate::pubsub::spawn_config_subscriber(client.clone());
    }

    let state = GatewayState::from_parts(
        config.clone(),
        catalog.clone(),
        std::sync::Arc::new(super::upstream::HyperUpstreamClient::new()),
        recorder,
    )?
    .with_api_keys(api_keys)
    .with_granted(granted)
    .with_spend_counter(spend_counter);
    let shared = SharedGatewayState::new(state, &config, config_path, catalog);
    Ok((
        build_router_with_shared(
            shared.clone(),
            std::sync::Arc::new(account_store),
            std::sync::Arc::new(catalog_store),
            quota_store,
            sessions,
            notifier,
        ),
        shared,
    ))
}

#[cfg(test)]
pub(crate) fn build_router_with_state(state: GatewayState) -> Router {
    build_router_with_shared(
        SharedGatewayState::without_reload(state),
        std::sync::Arc::new(crate::account::MemoryAccountStore::default()),
        std::sync::Arc::new(crate::catalog::MemoryCatalogStore::default()),
        std::sync::Arc::new(crate::quota::MemoryQuotaStore::default()),
        std::sync::Arc::new(crate::admin::MemorySessionStore::default()),
        None,
    )
}

pub(crate) fn build_router_with_shared(
    shared: SharedGatewayState,
    account_store: std::sync::Arc<dyn crate::account::AccountStore>,
    catalog_store: std::sync::Arc<dyn crate::catalog::CatalogStore>,
    quota_store: std::sync::Arc<dyn crate::quota::QuotaStore>,
    sessions: std::sync::Arc<dyn crate::admin::SessionStore>,
    notifier: Option<crate::pubsub::ConfigNotifier>,
) -> Router {
    // The admin console owns the /admin prefix, so nothing under it can ever
    // fall through to the upstream proxy.
    let admin = crate::admin::admin_router(
        &shared,
        account_store,
        catalog_store,
        quota_store,
        sessions,
        notifier,
    );
    // Model endpoints are fixed per protocol; everything else 404s instead of
    // being blindly proxied — the catalog is the routing contract.
    Router::new()
        .route("/healthz", get(healthz))
        .route("/reload", post(reload_endpoint))
        .route("/v1/chat/completions", post(proxy))
        .route("/v1/responses", post(proxy))
        .route("/v1/messages", post(proxy))
        .route("/v1/models", get(list_models))
        .nest_service("/admin", admin)
        .fallback(any(unknown_endpoint))
        .with_state(shared)
}

async fn unknown_endpoint(request: Request) -> Response {
    (
        StatusCode::NOT_FOUND,
        format!(
            "unknown endpoint {}; model endpoints are /v1/chat/completions, /v1/responses (OpenAI) and /v1/messages (Anthropic)",
            request.uri().path()
        ),
    )
        .into_response()
}

async fn healthz() -> &'static str {
    "ok"
}

async fn reload_endpoint(State(shared): State<SharedGatewayState>, headers: HeaderMap) -> Response {
    if shared.load().authorize(&headers).is_none() {
        warn!("rejected unauthenticated gateway reload request");
        return (
            StatusCode::UNAUTHORIZED,
            "missing or invalid API key",
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

    // The route table only sends fixed model-endpoint paths here.
    let protocol = protocol_of(&path).expect("proxy only registered on protocol paths");

    let Some(identity) = state.authorize(&original_headers) else {
        warn!(%request_id, %path, "rejected unauthenticated gateway request");
        return (
            StatusCode::UNAUTHORIZED,
            "missing or invalid API key",
        )
            .into_response();
    };

    // Soft prepaid-quota gate: two counter reads, zero database access. Soft
    // means in-flight requests may overshoot slightly; new requests stop here.
    match state.check_quota(&identity).await {
        crate::state::QuotaDecision::Allowed => {}
        crate::state::QuotaDecision::UserExhausted { granted } => {
            warn!(%request_id, %path, user_id = %identity.user_id, "rejected request: credit balance exhausted");
            return protocol_error(
                protocol,
                StatusCode::PAYMENT_REQUIRED,
                "insufficient_quota",
                format!(
                    "credit balance exhausted (granted {granted} USD); ask an administrator to grant more credit"
                ),
            );
        }
        crate::state::QuotaDecision::KeyCapReached { cap } => {
            warn!(%request_id, %path, api_key_id = %identity.api_key_id, "rejected request: key spend cap reached");
            return protocol_error(
                protocol,
                StatusCode::PAYMENT_REQUIRED,
                "insufficient_quota",
                format!(
                    "this API key reached its spend cap ({cap} USD); raise or remove the cap to continue"
                ),
            );
        }
    }

    let body = match to_bytes(request.into_body(), usize::MAX).await {
        Ok(body) => body,
        Err(error) => {
            warn!(%request_id, %path, error = %error, "failed to read request body");
            return (StatusCode::BAD_REQUEST, "failed to read request body").into_response();
        }
    };

    let payload = RequestPayload::parse(&body);

    let Some(model) = payload.model.clone() else {
        return protocol_error(
            protocol,
            StatusCode::BAD_REQUEST,
            "invalid_request_error",
            "the request body must include a string `model` field".to_owned(),
        );
    };

    // Resolution happens entirely in-memory, before any upstream contact:
    // unknown/disabled model, key restriction, and protocol filtering all
    // reject here with a protocol-native error body.
    let chain = match state.resolve(protocol, &model, &identity) {
        Resolution::Chain(chain) => chain,
        Resolution::UnknownModel => {
            warn!(%request_id, %path, %model, "requested model is not in the catalog");
            return protocol_error(
                protocol,
                StatusCode::NOT_FOUND,
                "not_found_error",
                format!("model '{model}' does not exist or is not available"),
            );
        }
        Resolution::Forbidden => {
            warn!(%request_id, %path, %model, "api key is not allowed to call model");
            return protocol_error(
                protocol,
                StatusCode::FORBIDDEN,
                "permission_error",
                format!("this API key is not allowed to call model '{model}'"),
            );
        }
        Resolution::WrongProtocol { available } => {
            warn!(%request_id, %path, %model, "model has no deployment on this protocol");
            let families = if available.is_empty() {
                "no protocol".to_owned()
            } else {
                available
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            return protocol_error(
                protocol,
                StatusCode::NOT_FOUND,
                "not_found_error",
                format!(
                    "model '{model}' is not available on this endpoint (protocol {protocol}); it is served via: {families}"
                ),
            );
        }
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
        identity,
        started_at,
    };

    let mut last_error: Option<Response> = None;

    let deployment_count = chain.len();
    for (index, deployment) in chain.iter().enumerate() {
        // The last deployment in the chain has nowhere to fail over to, so a
        // provider that exhausts its rate-limit retries forwards its real
        // response (e.g. 429) to the client instead of a synthesized 502.
        let is_last = index + 1 == deployment_count;
        match ctx
            .attempt_provider(deployment.provider, deployment.upstream_model, is_last)
            .await
        {
            ProviderAttempt::Forward(response) => return response,
            ProviderAttempt::Failover(response) => last_error = Some(response),
        }
    }

    last_error
        .unwrap_or_else(|| (StatusCode::BAD_GATEWAY, "upstream request failed").into_response())
}

/// A protocol-native JSON error body, so clients see the error shape their SDK
/// expects. `kind` follows Anthropic's error type vocabulary; the OpenAI shape
/// carries it as `code`.
fn protocol_error(
    protocol: Protocol,
    status: StatusCode,
    kind: &str,
    message: String,
) -> Response {
    let body = match protocol {
        Protocol::OpenAi => serde_json::json!({
            "error": {
                "message": message,
                "type": "invalid_request_error",
                "code": kind,
            }
        }),
        Protocol::Anthropic => serde_json::json!({
            "type": "error",
            "error": { "type": kind, "message": message }
        }),
    };
    (
        status,
        [(CONTENT_TYPE, "application/json")],
        body.to_string(),
    )
        .into_response()
}

/// Gateway-owned `GET /v1/models`: the enabled catalog scoped to the key. The
/// response shape follows the caller's endpoint family — Anthropic's when the
/// request carries an `anthropic-version` header, OpenAI's otherwise.
async fn list_models(State(shared): State<SharedGatewayState>, headers: HeaderMap) -> Response {
    let state = shared.load();
    let Some(identity) = state.authorize(&headers) else {
        return (
            StatusCode::UNAUTHORIZED,
            "missing or invalid API key",
        )
            .into_response();
    };

    let models = state.listable_models(&identity);
    let body = if headers.contains_key("anthropic-version") {
        serde_json::json!({
            "data": models
                .iter()
                .map(|(name, entry)| serde_json::json!({
                    "type": "model",
                    "id": name,
                    "display_name": name,
                    "created_at": entry.created_at.to_rfc3339(),
                }))
                .collect::<Vec<_>>(),
            "first_id": models.first().map(|(name, _)| *name),
            "last_id": models.last().map(|(name, _)| *name),
            "has_more": false,
        })
    } else {
        serde_json::json!({
            "object": "list",
            "data": models
                .iter()
                .map(|(name, entry)| serde_json::json!({
                    "id": name,
                    "object": "model",
                    "created": entry.created_at.timestamp(),
                    "owned_by": "lite-agentify",
                }))
                .collect::<Vec<_>>(),
        })
    };

    (
        StatusCode::OK,
        [(CONTENT_TYPE, "application/json")],
        body.to_string(),
    )
        .into_response()
}

/// The outcome of attempting a single deployment in a model's failover chain.
enum ProviderAttempt {
    /// A terminal response to forward to the client; no further deployments are tried.
    Forward(Response),
    /// A recoverable failure (transport error or HTTP 5xx); record and try the next deployment.
    Failover(Response),
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
    identity: crate::account::KeyIdentity,
    started_at: Instant,
}

impl ProxyContext<'_> {
    /// Attempts one deployment, retrying its provider in place on a configured
    /// rate-limit status (default 429/529) with backoff before giving up.
    /// `is_last` forwards an exhausted rate-limit response to the client rather
    /// than failing over to a non-existent next deployment.
    async fn attempt_provider(
        &self,
        provider: &Provider,
        upstream_model: &str,
        is_last: bool,
    ) -> ProviderAttempt {
        let request_id = self.request_id;
        let path = self.path;

        let provider_body = match body_for_deployment(self.body, self.payload, upstream_model) {
            Ok(body) => body,
            Err(error) => {
                warn!(%request_id, %path, provider = %provider.id, error = %error, "failed to rewrite request body for deployment");
                return ProviderAttempt::Forward(
                    (StatusCode::BAD_REQUEST, "failed to rewrite request body").into_response(),
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
            user_id: Some(self.identity.user_id),
            api_key_id: Some(self.identity.api_key_id),
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
    user_id: Option<uuid::Uuid>,
    api_key_id: Option<uuid::Uuid>,
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

    // Advance the spend counters off the response path so the next quota
    // check sees this request's cost. NULL costs (unpriced history) add zero.
    if let Some(cost) = estimated_cost {
        if let Some(user_id) = metadata.user_id {
            state
                .spend_counter
                .add(crate::quota::Scope::User(user_id), cost)
                .await;
        }
        if let Some(api_key_id) = metadata.api_key_id {
            state
                .spend_counter
                .add(crate::quota::Scope::Key(api_key_id), cost)
                .await;
        }
    }

    let record = UsageRecord {
        request_id: metadata.request_id,
        created_at: Utc::now(),
        provider_id: metadata.provider_id,
        protocol: metadata.protocol,
        path: metadata.path,
        user_id: metadata.user_id,
        api_key_id: metadata.api_key_id,
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

/// Rewrites the request body's top-level `model` to the deployment's upstream
/// name. When the requested name already matches, the original bytes pass
/// through untouched (no re-serialization).
fn body_for_deployment(
    body: &Bytes,
    payload: &RequestPayload,
    upstream_model: &str,
) -> anyhow::Result<ProviderBody> {
    let requested_model = payload.model.clone();
    if requested_model.as_deref() == Some(upstream_model) {
        return Ok(ProviderBody {
            body: body.clone(),
            requested_model,
            upstream_model: Some(upstream_model.to_owned()),
        });
    }

    // The proxy only resolves requests whose payload carried a model, so the
    // JSON value is present; guard defensively anyway.
    let Some(value) = payload.json.as_ref() else {
        bail!("request body is not a JSON object");
    };
    let mut value = value.clone();
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "model".to_owned(),
            Value::String(upstream_model.to_owned()),
        );
    }

    Ok(ProviderBody {
        body: Bytes::from(
            serde_json::to_vec(&value).context("failed to serialize JSON request body")?,
        ),
        requested_model,
        upstream_model: Some(upstream_model.to_owned()),
    })
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
