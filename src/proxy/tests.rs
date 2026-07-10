use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use axum::{
    body::{Body, Bytes, to_bytes},
    http::{
        HeaderMap, HeaderValue, Request as HttpRequest, Response, StatusCode,
        header::{AUTHORIZATION, CONTENT_TYPE},
    },
};
use rust_decimal::Decimal;
use tower::ServiceExt;

use super::{
    router::build_router_with_state,
    upstream::{UpstreamClient, UpstreamFuture, UpstreamRequest, UpstreamResponse},
};
use crate::{
    catalog::{CatalogSnapshot, DeploymentConfig, ModelConfig},
    config::{GatewayConfig, PricingConfig, ProviderConfig, RetryConfig},
    model::{Protocol, default_listen_addr},
    reload::{self, SharedGatewayState},
    state::GatewayState,
    usage::{MemoryUsageRecorder, NoopUsageRecorder, UsageRecorder},
};

/// Test shim: builds the router with an empty in-memory account store, so the
/// many router-construction call sites below stay one-argument. Production code
/// passes a real store from `build_router`.
fn build_router_with_shared(shared: SharedGatewayState) -> axum::Router {
    super::router::build_router_with_shared(
        shared,
        std::sync::Arc::new(crate::account::MemoryAccountStore::default()),
        std::sync::Arc::new(crate::catalog::MemoryCatalogStore::default()),
        std::sync::Arc::new(crate::quota::MemoryQuotaStore::default()),
        std::sync::Arc::new(crate::admin::MemorySessionStore::default()),
        None,
    )
}

#[derive(Default)]
struct RecordingClient {
    calls: AtomicUsize,
    requests: Mutex<Vec<UpstreamRequest>>,
    response_body: Mutex<Option<&'static str>>,
    response_content_type: Option<&'static str>,
}

impl RecordingClient {
    fn with_body(body: &'static str) -> Self {
        Self {
            response_body: Mutex::new(Some(body)),
            response_content_type: Some("text/event-stream"),
            ..Self::default()
        }
    }

    fn with_json_body(body: &'static str) -> Self {
        Self {
            response_body: Mutex::new(Some(body)),
            response_content_type: Some("application/json"),
            ..Self::default()
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn last_request(&self) -> UpstreamRequest {
        self.requests.lock().unwrap().last().unwrap().clone()
    }
}

impl UpstreamClient for RecordingClient {
    fn send(&self, request: UpstreamRequest) -> UpstreamFuture {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.requests.lock().unwrap().push(request);
        let body = self.response_body.lock().unwrap().take().unwrap_or("{}");
        let response_content_type = self.response_content_type;

        Box::pin(async move {
            let mut headers = HeaderMap::new();
            if let Some(content_type) = response_content_type {
                headers.insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
            }

            Ok(UpstreamResponse {
                status: StatusCode::OK,
                headers,
                body: Body::from(body),
            })
        })
    }
}

/// One scripted upstream outcome per call, consumed in order.
enum Outcome {
    Status(StatusCode),
    TransportError,
}

/// Upstream client that replays a fixed script of outcomes, recording the
/// upstream URI of every call so tests can assert which provider was contacted.
struct ScriptedClient {
    outcomes: Mutex<VecDeque<Outcome>>,
    uris: Mutex<Vec<String>>,
    bodies: Mutex<Vec<Bytes>>,
    calls: AtomicUsize,
}

impl ScriptedClient {
    fn new(outcomes: impl IntoIterator<Item = Outcome>) -> Self {
        Self {
            outcomes: Mutex::new(outcomes.into_iter().collect()),
            uris: Mutex::new(Vec::new()),
            bodies: Mutex::new(Vec::new()),
            calls: AtomicUsize::new(0),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn uris(&self) -> Vec<String> {
        self.uris.lock().unwrap().clone()
    }

    fn bodies(&self) -> Vec<Bytes> {
        self.bodies.lock().unwrap().clone()
    }
}

impl UpstreamClient for ScriptedClient {
    fn send(&self, request: UpstreamRequest) -> UpstreamFuture {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.uris.lock().unwrap().push(request.uri.to_string());
        self.bodies.lock().unwrap().push(request.body.clone());
        let outcome = self
            .outcomes
            .lock()
            .unwrap()
            .pop_front()
            .expect("ScriptedClient received more calls than scripted outcomes");

        Box::pin(async move {
            match outcome {
                Outcome::TransportError => Err(anyhow::anyhow!("simulated transport error")),
                Outcome::Status(status) => Ok(UpstreamResponse {
                    status,
                    headers: HeaderMap::new(),
                    body: Body::from("{}"),
                }),
            }
        })
    }
}

fn body_model(body: &[u8]) -> String {
    serde_json::from_slice::<serde_json::Value>(body)
        .unwrap()
        .get("model")
        .unwrap()
        .as_str()
        .unwrap()
        .to_owned()
}

/// An enabled catalog model with the given `(provider, upstream_model)` chain.
fn model(name: &str, deployments: &[(&str, &str)]) -> ModelConfig {
    ModelConfig {
        name: name.to_owned(),
        enabled: true,
        created_at: chrono::Utc::now(),
        deployments: deployments
            .iter()
            .map(|(provider_id, upstream_model)| DeploymentConfig {
                id: uuid::Uuid::new_v4(),
                provider_id: (*provider_id).to_owned(),
                upstream_model: (*upstream_model).to_owned(),
            })
            .collect(),
    }
}

/// Builds the gateway snapshot from a config plus catalog models, mirroring
/// what `build_router` assembles from the database at boot.
fn state_with_models(
    config: GatewayConfig,
    models: Vec<ModelConfig>,
    client: Arc<dyn UpstreamClient>,
) -> GatewayState {
    state_with_models_and_recorder(config, models, client, Arc::new(NoopUsageRecorder))
}

fn state_with_models_and_recorder(
    config: GatewayConfig,
    models: Vec<ModelConfig>,
    client: Arc<dyn UpstreamClient>,
    recorder: Arc<dyn UsageRecorder>,
) -> GatewayState {
    let catalog = CatalogSnapshot {
        providers: config.providers.clone(),
        pricing: config.pricing.clone(),
        models,
    };
    GatewayState::from_parts(config, catalog, client, recorder).unwrap()
}

/// Config with `primary` and `fallback` OpenAI providers; models are supplied
/// per test via `state_with_models`.
fn failover_config() -> GatewayConfig {
    GatewayConfig {
        listen_addr: default_listen_addr(),
        gateway_keys: vec!["gw-secret".to_owned()],
        admin_password: None,
        providers: vec![
            ProviderConfig {
                id: "primary".to_owned(),
                protocol: Protocol::OpenAi,
                base_url: "http://primary.test".to_owned(),
                api_key: "primary-secret".to_owned(),
                anthropic_version: None,
                model_aliases: std::collections::HashMap::new(),
            },
            ProviderConfig {
                id: "fallback".to_owned(),
                protocol: Protocol::OpenAi,
                base_url: "http://fallback.test".to_owned(),
                api_key: "fallback-secret".to_owned(),
                anthropic_version: None,
                model_aliases: std::collections::HashMap::new(),
            },
        ],
        routes: Vec::new(),
        database: None,
        redis: None,
        pricing: Vec::new(),
        retry: fast_retry(),
    }
}

/// The failover chain for `gpt-test` over `failover_config`'s two providers.
fn failover_models() -> Vec<ModelConfig> {
    vec![model(
        "gpt-test",
        &[("primary", "gpt-test"), ("fallback", "gpt-test")],
    )]
}

/// Retry policy for tests: keeps the default attempt count and statuses but
/// uses zero delays so retry paths never sleep in unit tests.
fn fast_retry() -> RetryConfig {
    RetryConfig {
        base_delay_ms: 0,
        max_delay_ms: 0,
        ..RetryConfig::default()
    }
}

async fn send_chat(app: axum::Router) -> Response<Body> {
    app_send_chat_with_model(app, "gpt-test").await
}

async fn app_send_chat_with_model(app: axum::Router, model: &str) -> Response<Body> {
    app.oneshot(
        HttpRequest::post("/v1/chat/completions")
            .header(AUTHORIZATION, "Bearer gw-secret")
            .body(Body::from(format!(r#"{{"model":"{model}"}}"#)))
            .unwrap(),
    )
    .await
    .unwrap()
}

fn config() -> GatewayConfig {
    GatewayConfig {
        listen_addr: default_listen_addr(),
        gateway_keys: vec!["gw-secret".to_owned()],
        admin_password: None,
        providers: vec![
            ProviderConfig {
                id: "openai".to_owned(),
                protocol: Protocol::OpenAi,
                base_url: "http://openai.test".to_owned(),
                api_key: "openai-secret".to_owned(),
                anthropic_version: None,
                model_aliases: std::collections::HashMap::new(),
            },
            ProviderConfig {
                id: "anthropic".to_owned(),
                protocol: Protocol::Anthropic,
                base_url: "http://anthropic.test".to_owned(),
                api_key: "anthropic-secret".to_owned(),
                anthropic_version: Some("2023-06-01".to_owned()),
                model_aliases: std::collections::HashMap::new(),
            },
        ],
        routes: Vec::new(),
        database: None,
        redis: None,
        pricing: Vec::new(),
        retry: fast_retry(),
    }
}

/// The standard catalog over `config()`: one OpenAI model and one Anthropic
/// model, each passing its public name upstream unchanged.
fn standard_models() -> Vec<ModelConfig> {
    vec![
        model("gpt-test", &[("openai", "gpt-test")]),
        model("claude-test", &[("anthropic", "claude-test")]),
    ]
}

fn priced_config() -> GatewayConfig {
    let mut config = config();
    config.pricing = vec![PricingConfig {
        provider: "openai".to_owned(),
        model: "gpt-real".to_owned(),
        input_per_1m: Decimal::new(200, 2),
        output_per_1m: Decimal::new(800, 2),
        cached_input_per_1m: Some(Decimal::new(50, 2)),
        cache_read_per_1m: None,
        cache_write_per_1m: None,
        currency: "USD".to_owned(),
        pricing_source: Some("test-pricing".to_owned()),
    }];
    config
}

/// `public-chat` → openai as `gpt-real`: the alias-style rename lives in the
/// deployment now.
fn renaming_models() -> Vec<ModelConfig> {
    vec![model("public-chat", &[("openai", "gpt-real")])]
}

// --- snapshot construction ---

#[test]
fn state_builds_without_config_gateway_keys() {
    // Authentication is database-backed: an empty gateway_keys list is not a
    // startup error (keys are managed in PostgreSQL). The resulting snapshot
    // simply has no API keys until the store populates them.
    let mut config = config();
    config.gateway_keys.clear();

    let state = GatewayState::from_config(config).expect("state builds without gateway keys");
    assert!(state.api_keys.is_empty());
}

#[test]
fn model_referencing_unknown_provider_fails_build() {
    let config = config();
    let catalog = CatalogSnapshot {
        providers: config.providers.clone(),
        pricing: Vec::new(),
        models: vec![model("gpt-test", &[("missing", "gpt-test")])],
    };
    assert!(
        GatewayState::from_parts(
            config,
            catalog,
            Arc::new(RecordingClient::default()),
            Arc::new(NoopUsageRecorder),
        )
        .is_err()
    );
}

#[test]
fn model_with_empty_upstream_name_fails_build() {
    let config = config();
    let catalog = CatalogSnapshot {
        providers: config.providers.clone(),
        pricing: Vec::new(),
        models: vec![model("gpt-test", &[("openai", "")])],
    };
    assert!(
        GatewayState::from_parts(
            config,
            catalog,
            Arc::new(RecordingClient::default()),
            Arc::new(NoopUsageRecorder),
        )
        .is_err()
    );
}

#[test]
fn empty_catalog_builds() {
    // A fresh install has no providers and no models; the gateway still boots
    // (serving the console) and requests resolve nothing.
    let mut config = config();
    config.providers.clear();
    let state = GatewayState::from_config(config).expect("empty catalog builds");
    assert!(state.models.is_empty());
}

#[test]
fn uses_configured_provider_credentials() {
    let state = GatewayState::from_config(config()).unwrap();
    assert_eq!(
        state.providers.get("openai").unwrap().api_key,
        "openai-secret"
    );
}

#[test]
fn loads_usage_database_and_pricing_config() {
    let parsed: GatewayConfig = toml::from_str(
        r#"
listen_addr = "127.0.0.1:3000"
gateway_keys = ["gw-secret"]

[usage_database]
enabled = true
url = "postgres://lite_agentify:password@localhost/lite_agentify"
max_connections = 5

[[providers]]
id = "openai"
protocol = "openai"
base_url = "http://openai.test"
api_key = "openai-secret"

[[routes]]
path_prefix = "/v1/chat/completions"
providers = ["openai"]

[[pricing]]
provider = "openai"
model = "gpt-real"
input_per_1m = "2.00"
output_per_1m = "8.00"
cached_input_per_1m = "0.50"
currency = "USD"
pricing_source = "manual-test"
"#,
    )
    .unwrap();

    let database = parsed.database.as_ref().unwrap();
    assert!(database.enabled);
    assert_eq!(database.max_connections, Some(5));
    assert_eq!(parsed.pricing[0].currency, "USD");
    // Legacy routes still parse (they feed the one-time catalog migration).
    assert_eq!(parsed.routes.len(), 1);
    assert!(GatewayState::from_config(parsed).is_ok());
}

#[test]
fn validates_pricing_entries() {
    let mut config_with_empty_provider = config();
    config_with_empty_provider.pricing = vec![PricingConfig {
        provider: "".to_owned(),
        model: "gpt-real".to_owned(),
        input_per_1m: Decimal::ONE,
        output_per_1m: Decimal::ONE,
        cached_input_per_1m: None,
        cache_read_per_1m: None,
        cache_write_per_1m: None,
        currency: "USD".to_owned(),
        pricing_source: None,
    }];
    assert!(GatewayState::from_config(config_with_empty_provider).is_err());

    let mut config_with_negative_price = config();
    config_with_negative_price.pricing = vec![PricingConfig {
        provider: "openai".to_owned(),
        model: "gpt-real".to_owned(),
        input_per_1m: Decimal::NEGATIVE_ONE,
        output_per_1m: Decimal::ONE,
        cached_input_per_1m: None,
        cache_read_per_1m: None,
        cache_write_per_1m: None,
        currency: "USD".to_owned(),
        pricing_source: None,
    }];
    assert!(GatewayState::from_config(config_with_negative_price).is_err());

    let mut config_with_bad_currency = config();
    config_with_bad_currency.pricing = vec![PricingConfig {
        provider: "openai".to_owned(),
        model: "gpt-real".to_owned(),
        input_per_1m: Decimal::ONE,
        output_per_1m: Decimal::ONE,
        cached_input_per_1m: None,
        cache_read_per_1m: None,
        cache_write_per_1m: None,
        currency: "usd".to_owned(),
        pricing_source: None,
    }];
    assert!(GatewayState::from_config(config_with_bad_currency).is_err());
}

// --- authentication ---

#[tokio::test]
async fn rejects_unauthenticated_request_before_upstream_contact() {
    let client = Arc::new(RecordingClient::default());
    let state = state_with_models(config(), standard_models(), client.clone());
    let app = build_router_with_state(state);

    let response = app
        .oneshot(
            HttpRequest::post("/v1/chat/completions")
                .body(Body::from(r#"{"model":"gpt-test"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(client.calls(), 0);
}

#[tokio::test]
async fn accepts_x_api_key_gateway_authentication() {
    let client = Arc::new(RecordingClient::default());
    let state = state_with_models(config(), standard_models(), client.clone());
    let app = build_router_with_state(state);

    let response = app
        .oneshot(
            HttpRequest::post("/v1/chat/completions")
                .header("x-api-key", "gw-secret")
                .body(Body::from(r#"{"model":"gpt-test"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(client.calls(), 1);
}

#[tokio::test]
async fn accepts_bare_authorization_gateway_authentication() {
    let client = Arc::new(RecordingClient::default());
    let state = state_with_models(config(), standard_models(), client.clone());
    let app = build_router_with_state(state);

    let response = app
        .oneshot(
            HttpRequest::post("/v1/chat/completions")
                .header(AUTHORIZATION, "gw-secret")
                .body(Body::from(r#"{"model":"gpt-test"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(client.calls(), 1);
}

// --- resolution ---

#[tokio::test]
async fn unknown_model_returns_protocol_native_404() {
    let client = Arc::new(RecordingClient::default());
    let state = state_with_models(config(), standard_models(), client.clone());

    let response =
        app_send_chat_with_model(build_router_with_state(state), "no-such-model").await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(client.calls(), 0);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    // OpenAI error shape, naming the model.
    assert!(json["error"]["message"].as_str().unwrap().contains("no-such-model"));
}

#[tokio::test]
async fn disabled_model_is_indistinguishable_from_unknown() {
    let client = Arc::new(RecordingClient::default());
    let mut models = standard_models();
    models[0].enabled = false;
    let state = state_with_models(config(), models, client.clone());

    let response = send_chat(build_router_with_state(state)).await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(client.calls(), 0);
}

#[tokio::test]
async fn missing_model_field_returns_bad_request() {
    let client = Arc::new(RecordingClient::default());
    let state = state_with_models(config(), standard_models(), client.clone());
    let app = build_router_with_state(state);

    let response = app
        .oneshot(
            HttpRequest::post("/v1/chat/completions")
                .header(AUTHORIZATION, "Bearer gw-secret")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(client.calls(), 0);
}

#[tokio::test]
async fn invalid_json_body_returns_bad_request() {
    let client = Arc::new(RecordingClient::default());
    let state = state_with_models(config(), standard_models(), client.clone());
    let app = build_router_with_state(state);

    let response = app
        .oneshot(
            HttpRequest::post("/v1/chat/completions")
                .header(AUTHORIZATION, "Bearer gw-secret")
                .body(Body::from("not json"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(client.calls(), 0);
}

#[tokio::test]
async fn filters_deployments_to_endpoint_protocol() {
    // claude-test only has an Anthropic deployment: calling it via the OpenAI
    // endpoint family 404s before any upstream contact, naming the protocol.
    let client = Arc::new(RecordingClient::default());
    let state = state_with_models(config(), standard_models(), client.clone());

    let response =
        app_send_chat_with_model(build_router_with_state(state), "claude-test").await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(client.calls(), 0);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json["error"]["message"].as_str().unwrap().contains("anthropic"));
}

#[tokio::test]
async fn mixed_protocol_model_serves_both_endpoint_families() {
    // One model deployed on both protocols: each endpoint family resolves to
    // its own provider. (Mixed chains were a startup error in the route era.)
    let client = Arc::new(RecordingClient::default());
    let models = vec![model(
        "dual",
        &[("openai", "gpt-dual"), ("anthropic", "claude-dual")],
    )];
    let state = state_with_models(config(), models, client.clone());
    let app = build_router_with_state(state);

    let response = app
        .clone()
        .oneshot(
            HttpRequest::post("/v1/messages")
                .header(AUTHORIZATION, "Bearer gw-secret")
                .body(Body::from(r#"{"model":"dual"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let request = client.last_request();
    assert!(request.uri.to_string().starts_with("http://anthropic.test"));
    assert_eq!(body_model(&request.body), "claude-dual");
}

#[tokio::test]
async fn key_restricted_to_other_models_gets_403() {
    let client = Arc::new(RecordingClient::default());
    let state = state_with_models(config(), standard_models(), client.clone());

    // Restrict the only key to claude-test, then call gpt-test.
    let mut api_keys = (*state.api_keys).clone();
    for identity in api_keys.values_mut() {
        identity.allowed_models = Some(Arc::new(
            ["claude-test".to_owned()].into_iter().collect(),
        ));
    }
    let state = state.with_api_keys(api_keys);

    let response = send_chat(build_router_with_state(state)).await;

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(client.calls(), 0);
}

#[tokio::test]
async fn key_allowed_model_passes_restriction() {
    let client = Arc::new(RecordingClient::default());
    let state = state_with_models(config(), standard_models(), client.clone());

    let mut api_keys = (*state.api_keys).clone();
    for identity in api_keys.values_mut() {
        identity.allowed_models =
            Some(Arc::new(["gpt-test".to_owned()].into_iter().collect()));
    }
    let state = state.with_api_keys(api_keys);

    let response = send_chat(build_router_with_state(state)).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(client.calls(), 1);
}

#[tokio::test]
async fn unknown_endpoint_is_not_proxied() {
    // Arbitrary paths are no longer forwarded upstream: only the fixed
    // protocol endpoints resolve models.
    let client = Arc::new(RecordingClient::default());
    let state = state_with_models(config(), standard_models(), client.clone());
    let app = build_router_with_state(state);

    let response = app
        .oneshot(
            HttpRequest::post("/v1/embeddings")
                .header(AUTHORIZATION, "Bearer gw-secret")
                .body(Body::from(r#"{"model":"gpt-test"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(client.calls(), 0);
}

// --- gateway-owned /v1/models ---

#[tokio::test]
async fn lists_models_scoped_to_key_in_openai_shape() {
    let client = Arc::new(RecordingClient::default());
    let mut models = standard_models();
    models.push(model("disabled-model", &[("openai", "x")]));
    models.last_mut().unwrap().enabled = false;
    let state = state_with_models(config(), models, client.clone());
    let app = build_router_with_state(state);

    let response = app
        .oneshot(
            HttpRequest::get("/v1/models")
                .header(AUTHORIZATION, "Bearer gw-secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(client.calls(), 0);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["object"], "list");
    let ids: Vec<&str> = json["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap())
        .collect();
    // Sorted, enabled-only; the disabled model never appears.
    assert_eq!(ids, vec!["claude-test", "gpt-test"]);
}

#[tokio::test]
async fn lists_models_in_anthropic_shape_for_anthropic_clients() {
    let client = Arc::new(RecordingClient::default());
    let state = state_with_models(config(), standard_models(), client);
    let app = build_router_with_state(state);

    let response = app
        .oneshot(
            HttpRequest::get("/v1/models")
                .header(AUTHORIZATION, "Bearer gw-secret")
                .header("anthropic-version", "2023-06-01")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["has_more"], false);
    assert_eq!(json["data"][0]["type"], "model");
}

#[tokio::test]
async fn model_listing_respects_key_restriction() {
    let client = Arc::new(RecordingClient::default());
    let state = state_with_models(config(), standard_models(), client);
    let mut api_keys = (*state.api_keys).clone();
    for identity in api_keys.values_mut() {
        identity.allowed_models =
            Some(Arc::new(["gpt-test".to_owned()].into_iter().collect()));
    }
    let state = state.with_api_keys(api_keys);
    let app = build_router_with_state(state);

    let response = app
        .oneshot(
            HttpRequest::get("/v1/models")
                .header(AUTHORIZATION, "Bearer gw-secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let ids: Vec<&str> = json["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["gpt-test"]);
}

#[tokio::test]
async fn model_listing_requires_authentication() {
    let client = Arc::new(RecordingClient::default());
    let state = state_with_models(config(), standard_models(), client);
    let app = build_router_with_state(state);

    let response = app
        .oneshot(HttpRequest::get("/v1/models").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

// --- proxying ---

#[tokio::test]
async fn attaches_openai_provider_credentials() {
    let client = Arc::new(RecordingClient::default());
    let state = state_with_models(config(), standard_models(), client.clone());
    let app = build_router_with_state(state);

    let response = app
        .oneshot(
            HttpRequest::post("/v1/chat/completions?foo=bar")
                .header(AUTHORIZATION, "Bearer gw-secret")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"model":"gpt-test"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let request = client.last_request();
    assert_eq!(
        request.uri.to_string(),
        "http://openai.test/v1/chat/completions?foo=bar"
    );
    assert_eq!(request.headers[AUTHORIZATION], "Bearer openai-secret");
}

#[tokio::test]
async fn rewrites_model_to_deployment_upstream_name() {
    let client = Arc::new(RecordingClient::default());
    let state = state_with_models(config(), renaming_models(), client.clone());
    let app = build_router_with_state(state);

    let response = app
        .oneshot(
            HttpRequest::post("/v1/chat/completions")
                .header(AUTHORIZATION, "Bearer gw-secret")
                .body(Body::from(r#"{"model":"public-chat"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_model(&client.last_request().body), "gpt-real");
}

#[tokio::test]
async fn same_name_deployment_preserves_original_body_bytes() {
    let client = Arc::new(RecordingClient::default());
    let state = state_with_models(config(), standard_models(), client.clone());
    let app = build_router_with_state(state);

    let response = app
        .oneshot(
            HttpRequest::post("/v1/chat/completions")
                .header(AUTHORIZATION, "Bearer gw-secret")
                .body(Body::from(r#"{"model":"gpt-test"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        client.last_request().body,
        Bytes::from_static(br#"{"model":"gpt-test"}"#)
    );
}

#[tokio::test]
async fn preserves_non_model_fields_and_provider_response() {
    let client = Arc::new(RecordingClient::with_body(
        r#"{"model":"provider-real","id":"response-1"}"#,
    ));
    let state = state_with_models(config(), renaming_models(), client.clone());
    let app = build_router_with_state(state);

    let response = app
        .oneshot(
            HttpRequest::post("/v1/chat/completions")
                .header(AUTHORIZATION, "Bearer gw-secret")
                .body(Body::from(
                    r#"{"model":"public-chat","temperature":0.2,"messages":[]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    let request_body =
        serde_json::from_slice::<serde_json::Value>(&client.last_request().body).unwrap();
    assert_eq!(request_body["model"], "gpt-real");
    assert_eq!(request_body["temperature"], 0.2);
    assert_eq!(request_body["messages"], serde_json::json!([]));

    let response_body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(
        response_body,
        Bytes::from_static(br#"{"model":"provider-real","id":"response-1"}"#)
    );
}

#[tokio::test]
async fn attaches_anthropic_provider_headers() {
    let client = Arc::new(RecordingClient::default());
    let state = state_with_models(config(), standard_models(), client.clone());
    let app = build_router_with_state(state);

    let response = app
        .oneshot(
            HttpRequest::post("/v1/messages")
                .header(AUTHORIZATION, "Bearer gw-secret")
                .body(Body::from(r#"{"model":"claude-test"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let request = client.last_request();
    assert_eq!(request.headers[AUTHORIZATION], "Bearer anthropic-secret");
    assert_eq!(request.headers["x-api-key"], "anthropic-secret");
    assert_eq!(request.headers["anthropic-version"], "2023-06-01");
}

#[tokio::test]
async fn forwards_client_identity_headers() {
    let client = Arc::new(RecordingClient::default());
    let state = state_with_models(config(), standard_models(), client.clone());
    let app = build_router_with_state(state);

    let response = app
        .oneshot(
            HttpRequest::post("/v1/messages")
                .header(AUTHORIZATION, "Bearer gw-secret")
                .header("user-agent", "claude-cli/2.1.181")
                .header("x-stainless-arch", "arm64")
                .header("x-app", "cli")
                .header("x-random-unlisted", "should-be-dropped")
                .body(Body::from(r#"{"model":"claude-test"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let request = client.last_request();
    assert_eq!(request.headers["user-agent"], "claude-cli/2.1.181");
    assert_eq!(request.headers["x-stainless-arch"], "arm64");
    assert_eq!(request.headers["x-app"], "cli");
    assert!(request.headers.get("x-random-unlisted").is_none());
}

#[tokio::test]
async fn forwards_streaming_bytes_without_rewriting() {
    let client = Arc::new(RecordingClient::with_body(
        "event: message\ndata: {\"x\":1}\n\n",
    ));
    let state = state_with_models(config(), standard_models(), client);
    let app = build_router_with_state(state);

    let response = app
        .oneshot(
            HttpRequest::post("/v1/chat/completions")
                .header(AUTHORIZATION, "Bearer gw-secret")
                .body(Body::from(r#"{"model":"gpt-test","stream":true}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();

    assert_eq!(
        body,
        Bytes::from_static(b"event: message\ndata: {\"x\":1}\n\n")
    );
}

// --- usage recording ---

#[tokio::test]
async fn records_non_streaming_usage_and_estimated_cost() {
    let client = Arc::new(RecordingClient::with_json_body(
        r#"{"id":"response-1","usage":{"prompt_tokens":1000,"completion_tokens":200,"total_tokens":1200,"prompt_tokens_details":{"cached_tokens":400}}}"#,
    ));
    let recorder = Arc::new(MemoryUsageRecorder::default());
    let state = state_with_models_and_recorder(
        priced_config(),
        renaming_models(),
        client,
        recorder.clone(),
    );

    let response = app_send_chat_with_model(build_router_with_state(state), "public-chat").await;
    let response_body = to_bytes(response.into_body(), usize::MAX).await.unwrap();

    assert_eq!(
        response_body,
        Bytes::from_static(
            br#"{"id":"response-1","usage":{"prompt_tokens":1000,"completion_tokens":200,"total_tokens":1200,"prompt_tokens_details":{"cached_tokens":400}}}"#
        )
    );
    let records = recorder.records();
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(record.provider_id, "openai");
    assert_eq!(record.requested_model.as_deref(), Some("public-chat"));
    assert_eq!(record.upstream_model.as_deref(), Some("gpt-real"));
    assert_eq!(record.input_tokens, Some(1000));
    assert_eq!(record.output_tokens, Some(200));
    assert_eq!(record.cached_input_tokens, Some(400));
    assert_eq!(record.estimated_cost, Some(Decimal::new(30, 4)));
    assert_eq!(record.currency.as_deref(), Some("USD"));
    assert_eq!(record.pricing_source.as_deref(), Some("test-pricing"));
}

#[tokio::test]
async fn records_streaming_usage_without_rewriting_stream() {
    let client = Arc::new(RecordingClient::with_body(
        "data: {\"choices\":[]}\n\ndata: {\"usage\":{\"prompt_tokens\":100,\"completion_tokens\":25,\"total_tokens\":125}}\n\ndata: [DONE]\n\n",
    ));
    let recorder = Arc::new(MemoryUsageRecorder::default());
    let mut config = config();
    config.pricing = vec![PricingConfig {
        provider: "openai".to_owned(),
        model: "gpt-test".to_owned(),
        input_per_1m: Decimal::new(200, 2),
        output_per_1m: Decimal::new(800, 2),
        cached_input_per_1m: None,
        cache_read_per_1m: None,
        cache_write_per_1m: None,
        currency: "USD".to_owned(),
        pricing_source: None,
    }];
    let state =
        state_with_models_and_recorder(config, standard_models(), client, recorder.clone());

    let response = send_chat(build_router_with_state(state)).await;
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();

    assert_eq!(
        body,
        Bytes::from_static(
            b"data: {\"choices\":[]}\n\ndata: {\"usage\":{\"prompt_tokens\":100,\"completion_tokens\":25,\"total_tokens\":125}}\n\ndata: [DONE]\n\n"
        )
    );
    let records = recorder.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].input_tokens, Some(100));
    assert_eq!(records[0].output_tokens, Some(25));
}

#[tokio::test]
async fn records_anthropic_streaming_usage_across_events() {
    let client = Arc::new(RecordingClient::with_body(
        "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":25,\"output_tokens\":1}}}\n\nevent: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":270}}\n\n",
    ));
    let recorder = Arc::new(MemoryUsageRecorder::default());
    let models = vec![model("claude", &[("anthropic", "claude")])];
    let state = state_with_models_and_recorder(config(), models, client, recorder.clone());

    let app = build_router_with_state(state);
    let response = app
        .oneshot(
            HttpRequest::post("/v1/messages")
                .header(AUTHORIZATION, "Bearer gw-secret")
                .body(Body::from(r#"{"model":"claude","stream":true}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let _ = to_bytes(response.into_body(), usize::MAX).await.unwrap();

    let records = recorder.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].input_tokens, Some(25));
    assert_eq!(records[0].output_tokens, Some(270));
    // Anthropic never reports a total; it is derived from the components.
    assert_eq!(records[0].total_tokens, Some(295));
}

#[tokio::test]
async fn streaming_without_usage_forwards_bytes_and_records_unavailable() {
    let client = Arc::new(RecordingClient::with_body(
        "event: message\ndata: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\ndata: [DONE]\n\n",
    ));
    let recorder = Arc::new(MemoryUsageRecorder::default());
    let state =
        state_with_models_and_recorder(config(), standard_models(), client, recorder.clone());

    let response = send_chat(build_router_with_state(state)).await;
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();

    assert_eq!(
        body,
        Bytes::from_static(
            b"event: message\ndata: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\ndata: [DONE]\n\n"
        )
    );
    let records = recorder.records();
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].usage_source,
        crate::usage::UsageSource::Unavailable
    );
    assert_eq!(records[0].input_tokens, None);
    assert_eq!(records[0].output_tokens, None);
}

#[tokio::test]
async fn persisted_usage_record_excludes_prompt_and_completion_content() {
    let client = Arc::new(RecordingClient::with_json_body(
        r#"{"choices":[{"message":{"content":"SECRET_COMPLETION"}}],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}"#,
    ));
    let recorder = Arc::new(MemoryUsageRecorder::default());
    let state = state_with_models_and_recorder(
        priced_config(),
        renaming_models(),
        client,
        recorder.clone(),
    );

    let app = build_router_with_state(state);
    let response = app
        .oneshot(
            HttpRequest::post("/v1/chat/completions")
                .header(AUTHORIZATION, "Bearer gw-secret")
                .body(Body::from(
                    r#"{"model":"public-chat","messages":[{"role":"user","content":"SECRET_PROMPT"}]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let _ = to_bytes(response.into_body(), usize::MAX).await.unwrap();

    let record_debug = format!("{:?}", recorder.records());
    assert!(!record_debug.contains("SECRET_PROMPT"));
    assert!(!record_debug.contains("SECRET_COMPLETION"));
}

#[tokio::test]
async fn usage_persistence_failure_does_not_alter_response() {
    let client = Arc::new(RecordingClient::with_json_body(
        r#"{"id":"response-1","usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}"#,
    ));
    let recorder = Arc::new(MemoryUsageRecorder::failing());
    let state =
        state_with_models_and_recorder(priced_config(), renaming_models(), client, recorder);

    let response = app_send_chat_with_model(build_router_with_state(state), "public-chat").await;
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        Bytes::from_static(
            br#"{"id":"response-1","usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}"#
        )
    );
}

#[tokio::test]
async fn healthz_works_without_authentication() {
    let client = Arc::new(RecordingClient::default());
    let state = state_with_models(config(), standard_models(), client);
    let app = build_router_with_state(state);

    let response = app
        .oneshot(HttpRequest::get("/healthz").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

// --- failover and retry over the deployment chain ---

#[tokio::test]
async fn primary_success_skips_fallback() {
    let client = Arc::new(ScriptedClient::new([Outcome::Status(StatusCode::OK)]));
    let state = state_with_models(failover_config(), failover_models(), client.clone());

    let response = send_chat(build_router_with_state(state)).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(client.calls(), 1);
    assert_eq!(
        client.uris(),
        vec!["http://primary.test/v1/chat/completions".to_owned()]
    );
}

#[tokio::test]
async fn primary_transport_error_fails_over_to_fallback() {
    let client = Arc::new(ScriptedClient::new([
        Outcome::TransportError,
        Outcome::Status(StatusCode::OK),
    ]));
    let state = state_with_models(failover_config(), failover_models(), client.clone());

    let response = send_chat(build_router_with_state(state)).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(client.calls(), 2);
    assert_eq!(
        client.uris(),
        vec![
            "http://primary.test/v1/chat/completions".to_owned(),
            "http://fallback.test/v1/chat/completions".to_owned(),
        ]
    );
}

#[tokio::test]
async fn each_deployment_rewrites_to_its_own_upstream_model() {
    let models = vec![model(
        "public-chat",
        &[("primary", "primary-real"), ("fallback", "fallback-real")],
    )];
    let client = Arc::new(ScriptedClient::new([
        Outcome::TransportError,
        Outcome::Status(StatusCode::OK),
    ]));
    let state = state_with_models(failover_config(), models, client.clone());

    let response = app_send_chat_with_model(build_router_with_state(state), "public-chat").await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        client
            .bodies()
            .iter()
            .map(|body| body_model(body))
            .collect::<Vec<_>>(),
        vec!["primary-real".to_owned(), "fallback-real".to_owned()]
    );
}

#[tokio::test]
async fn primary_server_error_fails_over_to_fallback() {
    let client = Arc::new(ScriptedClient::new([
        Outcome::Status(StatusCode::BAD_GATEWAY),
        Outcome::Status(StatusCode::OK),
    ]));
    let state = state_with_models(failover_config(), failover_models(), client.clone());

    let response = send_chat(build_router_with_state(state)).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(client.calls(), 2);
}

#[tokio::test]
async fn client_error_is_forwarded_without_failover() {
    let client = Arc::new(ScriptedClient::new([Outcome::Status(
        StatusCode::BAD_REQUEST,
    )]));
    let state = state_with_models(failover_config(), failover_models(), client.clone());

    let response = send_chat(build_router_with_state(state)).await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(client.calls(), 1);
}

#[tokio::test]
async fn rate_limit_retries_same_provider_then_succeeds() {
    // Primary returns 429 twice, then 200; retries stay on the primary and the
    // fallback is never contacted.
    let client = Arc::new(ScriptedClient::new([
        Outcome::Status(StatusCode::TOO_MANY_REQUESTS),
        Outcome::Status(StatusCode::TOO_MANY_REQUESTS),
        Outcome::Status(StatusCode::OK),
    ]));
    let state = state_with_models(failover_config(), failover_models(), client.clone());

    let response = send_chat(build_router_with_state(state)).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(client.calls(), 3);
    // Every attempt targeted the primary.
    assert!(
        client
            .uris()
            .iter()
            .all(|uri| uri.starts_with("http://primary.test"))
    );
}

#[tokio::test]
async fn rate_limit_exhausts_attempts_then_fails_over() {
    // Primary 429 for all 4 attempts, then fallback answers 200. Default
    // max_attempts is 4, so the primary is tried 4 times before advancing.
    let client = Arc::new(ScriptedClient::new([
        Outcome::Status(StatusCode::TOO_MANY_REQUESTS),
        Outcome::Status(StatusCode::TOO_MANY_REQUESTS),
        Outcome::Status(StatusCode::TOO_MANY_REQUESTS),
        Outcome::Status(StatusCode::TOO_MANY_REQUESTS),
        Outcome::Status(StatusCode::OK),
    ]));
    let state = state_with_models(failover_config(), failover_models(), client.clone());

    let response = send_chat(build_router_with_state(state)).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(client.calls(), 5);
    let uris = client.uris();
    assert_eq!(uris.iter().filter(|u| u.starts_with("http://primary.test")).count(), 4);
    assert_eq!(uris.iter().filter(|u| u.starts_with("http://fallback.test")).count(), 1);
}

#[tokio::test]
async fn rate_limit_single_provider_forwards_last_response() {
    // A single-deployment chain that only ever returns 429 forwards the real
    // 429 to the client after exhausting attempts, not a synthesized 502.
    let models = vec![model("gpt-test", &[("primary", "gpt-test")])];
    let client = Arc::new(ScriptedClient::new([
        Outcome::Status(StatusCode::TOO_MANY_REQUESTS),
        Outcome::Status(StatusCode::TOO_MANY_REQUESTS),
        Outcome::Status(StatusCode::TOO_MANY_REQUESTS),
        Outcome::Status(StatusCode::TOO_MANY_REQUESTS),
    ]));
    let state = state_with_models(failover_config(), models, client.clone());

    let response = send_chat(build_router_with_state(state)).await;

    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(client.calls(), 4);
}

#[tokio::test]
async fn overloaded_529_is_retryable_like_429() {
    // 529 is in the default retryable set; primary 529 then 200 succeeds on the
    // primary without failover.
    let client = Arc::new(ScriptedClient::new([
        Outcome::Status(StatusCode::from_u16(529).unwrap()),
        Outcome::Status(StatusCode::OK),
    ]));
    let state = state_with_models(failover_config(), failover_models(), client.clone());

    let response = send_chat(build_router_with_state(state)).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(client.calls(), 2);
    assert!(
        client
            .uris()
            .iter()
            .all(|uri| uri.starts_with("http://primary.test"))
    );
}

#[tokio::test]
async fn server_error_fails_over_without_same_provider_retry() {
    // 5xx still fails over immediately: primary is tried exactly once, then the
    // fallback answers. No same-provider retry for 5xx.
    let client = Arc::new(ScriptedClient::new([
        Outcome::Status(StatusCode::INTERNAL_SERVER_ERROR),
        Outcome::Status(StatusCode::OK),
    ]));
    let state = state_with_models(failover_config(), failover_models(), client.clone());

    let response = send_chat(build_router_with_state(state)).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(client.calls(), 2);
    let uris = client.uris();
    assert_eq!(uris.iter().filter(|u| u.starts_with("http://primary.test")).count(), 1);
    assert_eq!(uris.iter().filter(|u| u.starts_with("http://fallback.test")).count(), 1);
}

#[tokio::test]
async fn non_retryable_client_error_is_forwarded_immediately() {
    // A 4xx that is not in the retryable set is forwarded on the first try with
    // no retry and no failover.
    let client = Arc::new(ScriptedClient::new([Outcome::Status(
        StatusCode::BAD_REQUEST,
    )]));
    let state = state_with_models(failover_config(), failover_models(), client.clone());

    let response = send_chat(build_router_with_state(state)).await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(client.calls(), 1);
}

#[test]
fn retry_config_defaults_apply_when_absent() {
    // The default RetryConfig is valid and yields the documented defaults.
    let config = failover_config();
    // Sanity: fast_retry keeps the default attempts/statuses.
    assert_eq!(config.retry.max_attempts, 4);
    assert!(config.retry.retryable_statuses.contains(&429));
    assert!(config.retry.retryable_statuses.contains(&529));
    assert!(GatewayState::from_config(config).is_ok());
}

#[test]
fn retry_config_rejects_zero_attempts() {
    let mut config = failover_config();
    config.retry.max_attempts = 0;
    assert!(GatewayState::from_config(config).is_err());
}

#[test]
fn retry_config_rejects_base_delay_above_max() {
    let mut config = failover_config();
    config.retry.base_delay_ms = 5000;
    config.retry.max_delay_ms = 1000;
    assert!(GatewayState::from_config(config).is_err());
}

#[test]
fn retry_backoff_is_capped_and_retry_after_bounded() {
    // The computed backoff never exceeds max_delay, and a large Retry-After is
    // capped to max_delay by the policy.
    let policy = crate::model::RetryPolicy {
        retryable_statuses: [429u16, 529].into_iter().collect(),
        max_attempts: 4,
        base_delay_ms: 1000,
        max_delay_ms: 8000,
    };
    // Full-jitter draw with fraction 1.0 hits the ceiling; even a high attempt
    // index stays capped at max_delay.
    assert!(policy.backoff_ms(0, 1.0) <= 8000);
    assert!(policy.backoff_ms(10, 1.0) <= 8000);
    // Jitter of 0 yields no wait.
    assert_eq!(policy.backoff_ms(3, 0.0), 0);
    // A 300s Retry-After is capped to max_delay.
    assert_eq!(policy.cap_delay_ms(300_000), 8000);
}

#[tokio::test]
async fn exhausted_chain_returns_gateway_error() {
    let client = Arc::new(ScriptedClient::new([
        Outcome::Status(StatusCode::INTERNAL_SERVER_ERROR),
        Outcome::TransportError,
    ]));
    let state = state_with_models(failover_config(), failover_models(), client.clone());

    let response = send_chat(build_router_with_state(state)).await;

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(client.calls(), 2);
}

#[tokio::test]
async fn enabled_model_without_deployments_resolves_nothing() {
    // The admin API refuses to enable a deployment-less model, but a stale
    // snapshot could still carry one: requests get a clean 404, not a panic.
    let client = Arc::new(RecordingClient::default());
    let models = vec![model("gpt-test", &[])];
    let state = state_with_models(failover_config(), models, client.clone());

    let response = send_chat(build_router_with_state(state)).await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(client.calls(), 0);
}

// --- config hot reload ---

/// File contents whose reloadable field (retry) differs from `fast_retry`, and
/// whose gateway key differs so tests can observe what a reload does and does
/// not apply.
const RELOADED_CONFIG_TOML: &str = r#"
listen_addr = "127.0.0.1:9"
gateway_keys = ["gw-secret-v2"]

[retry]
retryable_statuses = [429]
max_attempts = 2
base_delay_ms = 0
max_delay_ms = 0
"#;

fn write_temp_config(contents: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "lite-agentify-reload-test-{}.toml",
        uuid::Uuid::new_v4()
    ));
    std::fs::write(&path, contents).unwrap();
    path
}

fn shared_state_with_config_file(
    client: Arc<dyn UpstreamClient>,
    contents: &str,
) -> SharedGatewayState {
    let base = config();
    let models = standard_models();
    let catalog = CatalogSnapshot {
        providers: base.providers.clone(),
        pricing: base.pricing.clone(),
        models,
    };
    // Seed the catalog (providers/pricing/models) exactly as boot does, so a
    // reload — which overlays the cached catalog — keeps resolving models.
    let state = GatewayState::from_parts(
        base.clone(),
        catalog.clone(),
        client,
        Arc::new(NoopUsageRecorder),
    )
    .unwrap();
    SharedGatewayState::new(state, &base, write_temp_config(contents), catalog)
}

async fn send_chat_with_key(app: axum::Router, key: &str, model: &str) -> Response<Body> {
    app.oneshot(
        HttpRequest::post("/v1/chat/completions")
            .header(AUTHORIZATION, format!("Bearer {key}"))
            .body(Body::from(format!(r#"{{"model":"{model}"}}"#)))
            .unwrap(),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn reload_applies_new_configuration() {
    let client = Arc::new(RecordingClient::default());
    let shared = shared_state_with_config_file(client.clone(), RELOADED_CONFIG_TOML);

    reload::reload(&shared).unwrap();

    // A file reload re-reads retry only; providers, pricing, and models come
    // from the database catalog and keys stay database-owned. The catalog
    // (seeded from config()) still routes gpt-test to the openai provider, and
    // the carried-over key `gw-secret` still authenticates.
    assert_eq!(shared.load().retry_policy.max_attempts, 2);
    let app = build_router_with_shared(shared.clone());
    let response = send_chat_with_key(app, "gw-secret", "gpt-test").await;
    assert_eq!(response.status(), StatusCode::OK);
    let request = client.last_request();
    assert!(request.uri.to_string().starts_with("http://openai.test"));

    // The file-only key was never imported, so it does not authenticate.
    let app = build_router_with_shared(shared);
    let response = send_chat_with_key(app, "gw-secret-v2", "gpt-test").await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn reload_with_invalid_toml_keeps_previous_configuration() {
    let client = Arc::new(RecordingClient::default());
    let shared = shared_state_with_config_file(client, "not valid toml ][");

    assert!(reload::reload(&shared).is_err());

    let response = send_chat(build_router_with_shared(shared)).await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn reload_with_failing_validation_keeps_previous_configuration() {
    // Parses as TOML but the retry policy is invalid.
    let invalid = r#"
[retry]
max_attempts = 0
"#;
    let client = Arc::new(RecordingClient::default());
    let shared = shared_state_with_config_file(client, invalid);

    assert!(reload::reload(&shared).is_err());

    let response = send_chat(build_router_with_shared(shared)).await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn reload_ignores_listen_addr_and_database_changes() {
    // Changed listen_addr and a new database block are warned about and
    // skipped, while the reloadable fields still take effect.
    let with_non_reloadable_changes =
        format!("{RELOADED_CONFIG_TOML}\n[database]\nurl = \"postgres://db.test/usage\"\n");
    let client = Arc::new(RecordingClient::default());
    let shared = shared_state_with_config_file(client, &with_non_reloadable_changes);

    reload::reload(&shared).unwrap();

    // Keys are database-owned; the carried-over key still authenticates.
    let response = send_chat_with_key(
        build_router_with_shared(shared),
        "gw-secret",
        "gpt-test",
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
}

async fn post_reload(app: axum::Router, key: Option<&str>) -> Response<Body> {
    let mut request = HttpRequest::post("/reload");
    if let Some(key) = key {
        request = request.header(AUTHORIZATION, format!("Bearer {key}"));
    }
    app.oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn reload_endpoint_applies_new_configuration() {
    let client = Arc::new(RecordingClient::default());
    let shared = shared_state_with_config_file(client, RELOADED_CONFIG_TOML);

    let response = post_reload(build_router_with_shared(shared.clone()), Some("gw-secret")).await;
    assert_eq!(response.status(), StatusCode::OK);

    // Reload succeeded; providers stay catalog-sourced and keys stay
    // database-owned, so the carried-over key authenticates as before.
    let response = send_chat_with_key(
        build_router_with_shared(shared),
        "gw-secret",
        "gpt-test",
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn reload_endpoint_reports_failure_and_keeps_serving() {
    let client = Arc::new(RecordingClient::default());
    let shared = shared_state_with_config_file(client, "not valid toml ][");

    let response = post_reload(build_router_with_shared(shared.clone()), Some("gw-secret")).await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let response = send_chat(build_router_with_shared(shared)).await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn reload_endpoint_rejects_unauthenticated_request() {
    let client = Arc::new(RecordingClient::default());
    let shared = shared_state_with_config_file(client, RELOADED_CONFIG_TOML);

    let response = post_reload(build_router_with_shared(shared.clone()), None).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    // The reload must not have been triggered: the old key still works.
    let response = send_chat(build_router_with_shared(shared)).await;
    assert_eq!(response.status(), StatusCode::OK);
}

// --- catalog snapshot refresh (store_catalog) ---

#[tokio::test]
async fn store_catalog_applies_model_changes_without_file_reload() {
    let client = Arc::new(RecordingClient::default());
    let shared = shared_state_with_config_file(client.clone(), RELOADED_CONFIG_TOML);

    // Add a new model to the catalog and refresh.
    let mut catalog = (*shared.catalog()).clone();
    catalog.models.push(model("new-model", &[("openai", "gpt-new")]));
    shared.store_catalog(catalog).unwrap();

    let response = app_send_chat_with_model(
        build_router_with_shared(shared),
        "new-model",
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_model(&client.last_request().body), "gpt-new");
}

#[tokio::test]
async fn store_catalog_rejects_model_referencing_missing_provider() {
    let client = Arc::new(RecordingClient::default());
    let shared = shared_state_with_config_file(client, RELOADED_CONFIG_TOML);

    let mut catalog = (*shared.catalog()).clone();
    catalog.models.push(model("broken", &[("nope", "x")]));
    assert!(shared.store_catalog(catalog).is_err());

    // The previous snapshot keeps serving.
    let response = send_chat(build_router_with_shared(shared)).await;
    assert_eq!(response.status(), StatusCode::OK);
}
