use std::{
    collections::{HashMap, VecDeque},
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
    config::{GatewayConfig, PricingConfig, ProviderConfig, RouteConfig},
    model::{Protocol, default_listen_addr},
    reload::{self, SharedGatewayState},
    router::{build_router_with_shared, build_router_with_state},
    state::GatewayState,
    upstream::{UpstreamClient, UpstreamFuture, UpstreamRequest, UpstreamResponse},
    usage::MemoryUsageRecorder,
};

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

fn aliases(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(alias, upstream)| ((*alias).to_owned(), (*upstream).to_owned()))
        .collect()
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

/// Config with a single OpenAI-compatible route whose chain is `[primary, fallback]`.
fn failover_config() -> GatewayConfig {
    GatewayConfig {
        listen_addr: default_listen_addr(),
        gateway_keys: vec!["gw-secret".to_owned()],
        providers: vec![
            ProviderConfig {
                id: "primary".to_owned(),
                protocol: Protocol::OpenAi,
                base_url: "http://primary.test".to_owned(),
                api_key: "primary-secret".to_owned(),
                anthropic_version: None,
                model_aliases: HashMap::new(),
            },
            ProviderConfig {
                id: "fallback".to_owned(),
                protocol: Protocol::OpenAi,
                base_url: "http://fallback.test".to_owned(),
                api_key: "fallback-secret".to_owned(),
                anthropic_version: None,
                model_aliases: HashMap::new(),
            },
        ],
        routes: vec![RouteConfig {
            path_prefix: "/v1/chat/completions".to_owned(),
            providers: vec!["primary".to_owned(), "fallback".to_owned()],
            model_prefix: None,
        }],
        usage_database: None,
        pricing: Vec::new(),
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
        providers: vec![
            ProviderConfig {
                id: "openai".to_owned(),
                protocol: Protocol::OpenAi,
                base_url: "http://openai.test".to_owned(),
                api_key: "openai-secret".to_owned(),
                anthropic_version: None,
                model_aliases: HashMap::new(),
            },
            ProviderConfig {
                id: "anthropic".to_owned(),
                protocol: Protocol::Anthropic,
                base_url: "http://anthropic.test".to_owned(),
                api_key: "anthropic-secret".to_owned(),
                anthropic_version: Some("2023-06-01".to_owned()),
                model_aliases: HashMap::new(),
            },
        ],
        routes: vec![
            RouteConfig {
                path_prefix: "/v1/chat/completions".to_owned(),
                providers: vec!["openai".to_owned()],
                model_prefix: None,
            },
            RouteConfig {
                path_prefix: "/v1/messages".to_owned(),
                providers: vec!["anthropic".to_owned()],
                model_prefix: None,
            },
        ],
        usage_database: None,
        pricing: Vec::new(),
    }
}

fn priced_config() -> GatewayConfig {
    let mut config = config();
    config.providers[0].model_aliases = aliases(&[("public-chat", "gpt-real")]);
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

#[test]
fn validates_missing_gateway_keys() {
    let mut config = config();
    config.gateway_keys.clear();

    assert!(GatewayState::from_config(config).is_err());
}

#[test]
fn skips_routes_for_missing_providers() {
    let mut config = config();
    config
        .providers
        .retain(|provider| provider.id != "anthropic");

    let state = GatewayState::from_config(config).unwrap();

    assert!(state.match_route("/v1/messages", None).is_none());
    assert!(state.match_route("/v1/chat/completions", None).is_some());
}

#[test]
fn fails_when_no_routes_reference_configured_providers() {
    let mut config = config();
    for route in &mut config.routes {
        route.providers = vec!["missing".to_owned()];
    }

    assert!(GatewayState::from_config(config).is_err());
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
fn validates_provider_model_aliases() {
    let mut config_with_empty_alias = config();
    config_with_empty_alias.providers[0].model_aliases = aliases(&[("", "gpt-real")]);
    assert!(GatewayState::from_config(config_with_empty_alias).is_err());

    let mut config_with_empty_upstream = config();
    config_with_empty_upstream.providers[0].model_aliases = aliases(&[("public-chat", "")]);
    assert!(GatewayState::from_config(config_with_empty_upstream).is_err());
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

    let database = parsed.usage_database.as_ref().unwrap();
    assert!(database.enabled);
    assert_eq!(database.max_connections, Some(5));
    assert_eq!(parsed.pricing[0].currency, "USD");
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

#[test]
fn matches_model_prefix_route() {
    let mut config = config();
    config.providers.push(ProviderConfig {
        id: "deepseek".to_owned(),
        protocol: Protocol::OpenAi,
        base_url: "http://deepseek.test".to_owned(),
        api_key: "deepseek-secret".to_owned(),
        anthropic_version: None,
        model_aliases: HashMap::new(),
    });
    config.routes.insert(
        0,
        RouteConfig {
            path_prefix: "/v1/chat/completions".to_owned(),
            providers: vec!["deepseek".to_owned()],
            model_prefix: Some("deepseek-".to_owned()),
        },
    );

    let state = GatewayState::from_config(config).unwrap();
    let route = state
        .match_route("/v1/chat/completions", Some("deepseek-chat"))
        .unwrap();

    assert_eq!(route.provider_ids, vec!["deepseek".to_owned()]);
}

#[tokio::test]
async fn rejects_unauthenticated_request_before_upstream_contact() {
    let client = Arc::new(RecordingClient::default());
    let state = GatewayState::from_config_with_upstream(config(), client.clone()).unwrap();
    let app = build_router_with_state(state);

    let response = app
        .oneshot(
            HttpRequest::post("/v1/chat/completions")
                .body(Body::from("{}"))
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
    let state = GatewayState::from_config_with_upstream(config(), client.clone()).unwrap();
    let app = build_router_with_state(state);

    let response = app
        .oneshot(
            HttpRequest::post("/v1/chat/completions")
                .header("x-api-key", "gw-secret")
                .body(Body::from("{}"))
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
    let state = GatewayState::from_config_with_upstream(config(), client.clone()).unwrap();
    let app = build_router_with_state(state);

    let response = app
        .oneshot(
            HttpRequest::post("/v1/chat/completions")
                .header(AUTHORIZATION, "gw-secret")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(client.calls(), 1);
}

#[tokio::test]
async fn returns_route_error_without_cross_protocol_conversion() {
    let client = Arc::new(RecordingClient::default());
    let state = GatewayState::from_config_with_upstream(config(), client.clone()).unwrap();
    let app = build_router_with_state(state);

    let response = app
        .oneshot(
            HttpRequest::post("/v1/responses")
                .header(AUTHORIZATION, "Bearer gw-secret")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(client.calls(), 0);
}

#[tokio::test]
async fn attaches_openai_provider_credentials() {
    let client = Arc::new(RecordingClient::default());
    let state = GatewayState::from_config_with_upstream(config(), client.clone()).unwrap();
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
async fn rewrites_model_alias_for_selected_provider() {
    let mut config = config();
    config.providers[0].model_aliases = aliases(&[("public-chat", "gpt-real")]);

    let client = Arc::new(RecordingClient::default());
    let state = GatewayState::from_config_with_upstream(config, client.clone()).unwrap();
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
async fn alias_provider_with_invalid_json_body_returns_bad_request() {
    let mut config = config();
    config.providers[0].model_aliases = aliases(&[("public-chat", "gpt-real")]);

    let client = Arc::new(RecordingClient::default());
    let state = GatewayState::from_config_with_upstream(config, client.clone()).unwrap();
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
async fn provider_without_aliases_preserves_original_model() {
    let client = Arc::new(RecordingClient::default());
    let state = GatewayState::from_config_with_upstream(config(), client.clone()).unwrap();
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
    let mut config = config();
    config.providers[0].model_aliases = aliases(&[("public-chat", "gpt-real")]);
    let client = Arc::new(RecordingClient::with_body(
        r#"{"model":"provider-real","id":"response-1"}"#,
    ));
    let state = GatewayState::from_config_with_upstream(config, client.clone()).unwrap();
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
    let state = GatewayState::from_config_with_upstream(config(), client.clone()).unwrap();
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
    let state = GatewayState::from_config_with_upstream(config(), client.clone()).unwrap();
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
    let state = GatewayState::from_config_with_upstream(config(), client).unwrap();
    let app = build_router_with_state(state);

    let response = app
        .oneshot(
            HttpRequest::post("/v1/chat/completions")
                .header(AUTHORIZATION, "Bearer gw-secret")
                .body(Body::from(r#"{"stream":true}"#))
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

#[tokio::test]
async fn records_non_streaming_usage_and_estimated_cost() {
    let client = Arc::new(RecordingClient::with_json_body(
        r#"{"id":"response-1","usage":{"prompt_tokens":1000,"completion_tokens":200,"total_tokens":1200,"prompt_tokens_details":{"cached_tokens":400}}}"#,
    ));
    let recorder = Arc::new(MemoryUsageRecorder::default());
    let state = GatewayState::from_config_with_upstream_and_recorder(
        priced_config(),
        client,
        recorder.clone(),
    )
    .unwrap();

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
        GatewayState::from_config_with_upstream_and_recorder(config, client, recorder.clone())
            .unwrap();

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
    let state =
        GatewayState::from_config_with_upstream_and_recorder(config(), client, recorder.clone())
            .unwrap();

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
}

#[tokio::test]
async fn streaming_without_usage_forwards_bytes_and_records_unavailable() {
    let client = Arc::new(RecordingClient::with_body(
        "event: message\ndata: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\ndata: [DONE]\n\n",
    ));
    let recorder = Arc::new(MemoryUsageRecorder::default());
    let state =
        GatewayState::from_config_with_upstream_and_recorder(config(), client, recorder.clone())
            .unwrap();

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
        super::usage::UsageSource::Unavailable
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
    let state = GatewayState::from_config_with_upstream_and_recorder(
        priced_config(),
        client,
        recorder.clone(),
    )
    .unwrap();

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
        GatewayState::from_config_with_upstream_and_recorder(priced_config(), client, recorder)
            .unwrap();

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
    let state = GatewayState::from_config_with_upstream(config(), client).unwrap();
    let app = build_router_with_state(state);

    let response = app
        .oneshot(HttpRequest::get("/healthz").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn primary_success_skips_fallback() {
    let client = Arc::new(ScriptedClient::new([Outcome::Status(StatusCode::OK)]));
    let state = GatewayState::from_config_with_upstream(failover_config(), client.clone()).unwrap();

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
    let state = GatewayState::from_config_with_upstream(failover_config(), client.clone()).unwrap();

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
async fn fallback_provider_receives_its_own_model_alias() {
    let mut config = failover_config();
    config.providers[0].model_aliases = aliases(&[("public-chat", "primary-real")]);
    config.providers[1].model_aliases = aliases(&[("public-chat", "fallback-real")]);
    let client = Arc::new(ScriptedClient::new([
        Outcome::TransportError,
        Outcome::Status(StatusCode::OK),
    ]));
    let state = GatewayState::from_config_with_upstream(config, client.clone()).unwrap();

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
async fn skips_alias_enabled_provider_without_requested_alias() {
    let mut config = failover_config();
    config.providers[0].model_aliases = aliases(&[("other-chat", "primary-real")]);
    config.providers[1].model_aliases = aliases(&[("public-chat", "fallback-real")]);
    let client = Arc::new(ScriptedClient::new([Outcome::Status(StatusCode::OK)]));
    let state = GatewayState::from_config_with_upstream(config, client.clone()).unwrap();

    let response = app_send_chat_with_model(build_router_with_state(state), "public-chat").await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(client.calls(), 1);
    assert_eq!(
        client.uris(),
        vec!["http://fallback.test/v1/chat/completions".to_owned()]
    );
    assert_eq!(body_model(&client.bodies()[0]), "fallback-real");
}

#[tokio::test]
async fn returns_gateway_error_when_no_provider_resolves_alias() {
    let mut config = failover_config();
    config.providers[0].model_aliases = aliases(&[("other-chat", "primary-real")]);
    config.providers[1].model_aliases = aliases(&[("other-chat", "fallback-real")]);
    let client = Arc::new(RecordingClient::default());
    let state = GatewayState::from_config_with_upstream(config, client.clone()).unwrap();

    let response = app_send_chat_with_model(build_router_with_state(state), "public-chat").await;

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(client.calls(), 0);
}

#[tokio::test]
async fn primary_server_error_fails_over_to_fallback() {
    let client = Arc::new(ScriptedClient::new([
        Outcome::Status(StatusCode::BAD_GATEWAY),
        Outcome::Status(StatusCode::OK),
    ]));
    let state = GatewayState::from_config_with_upstream(failover_config(), client.clone()).unwrap();

    let response = send_chat(build_router_with_state(state)).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(client.calls(), 2);
}

#[tokio::test]
async fn client_error_is_forwarded_without_failover() {
    let client = Arc::new(ScriptedClient::new([Outcome::Status(
        StatusCode::BAD_REQUEST,
    )]));
    let state = GatewayState::from_config_with_upstream(failover_config(), client.clone()).unwrap();

    let response = send_chat(build_router_with_state(state)).await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(client.calls(), 1);
}

#[tokio::test]
async fn rate_limit_is_forwarded_without_failover() {
    let client = Arc::new(ScriptedClient::new([Outcome::Status(
        StatusCode::TOO_MANY_REQUESTS,
    )]));
    let state = GatewayState::from_config_with_upstream(failover_config(), client.clone()).unwrap();

    let response = send_chat(build_router_with_state(state)).await;

    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(client.calls(), 1);
}

#[tokio::test]
async fn exhausted_chain_returns_gateway_error() {
    let client = Arc::new(ScriptedClient::new([
        Outcome::Status(StatusCode::INTERNAL_SERVER_ERROR),
        Outcome::TransportError,
    ]));
    let state = GatewayState::from_config_with_upstream(failover_config(), client.clone()).unwrap();

    let response = send_chat(build_router_with_state(state)).await;

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(client.calls(), 2);
}

#[test]
fn mixed_protocol_chain_fails_startup() {
    let mut config = failover_config();
    // Make the fallback a different protocol than the primary.
    config
        .providers
        .iter_mut()
        .find(|provider| provider.id == "fallback")
        .unwrap()
        .protocol = Protocol::Anthropic;

    assert!(GatewayState::from_config(config).is_err());
}

#[test]
fn empty_provider_chain_fails_startup() {
    let mut config = failover_config();
    config.routes[0].providers.clear();

    assert!(GatewayState::from_config(config).is_err());
}

// --- config hot reload ---

/// Valid config whose gateway key, provider base URL, and alias differ from
/// `config()` so tests can observe that a reload took effect.
const RELOADED_CONFIG_TOML: &str = r#"
listen_addr = "127.0.0.1:9"
gateway_keys = ["gw-secret-v2"]

[[providers]]
id = "openai"
protocol = "openai"
base_url = "http://openai-v2.test"
api_key = "openai-secret"

[providers.model_aliases]
public-chat = "gpt-v2"

[[routes]]
path_prefix = "/v1/chat/completions"
providers = ["openai"]
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
    let state = GatewayState::from_config_with_upstream(config(), client).unwrap();
    SharedGatewayState::new(state, &config(), write_temp_config(contents))
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

    // New key, provider URL, and alias are live; the old key is rejected.
    let app = build_router_with_shared(shared.clone());
    let response = send_chat_with_key(app, "gw-secret-v2", "public-chat").await;
    assert_eq!(response.status(), StatusCode::OK);
    let request = client.last_request();
    assert!(request.uri.to_string().starts_with("http://openai-v2.test"));
    assert_eq!(body_model(&request.body), "gpt-v2");

    let app = build_router_with_shared(shared);
    let response = send_chat_with_key(app, "gw-secret", "public-chat").await;
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
    // Parses as TOML but the route references an unknown provider.
    let invalid = r#"
gateway_keys = ["gw-secret-v2"]

[[providers]]
id = "openai"
protocol = "openai"
base_url = "http://openai-v2.test"
api_key = "openai-secret"

[[routes]]
path_prefix = "/v1/chat/completions"
providers = ["missing-provider"]
"#;
    let client = Arc::new(RecordingClient::default());
    let shared = shared_state_with_config_file(client, invalid);

    assert!(reload::reload(&shared).is_err());

    let response = send_chat(build_router_with_shared(shared)).await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn reload_ignores_listen_addr_and_usage_database_changes() {
    // Changed listen_addr and a new usage_database block are warned about and
    // skipped, while the reloadable fields still take effect.
    let with_non_reloadable_changes =
        format!("{RELOADED_CONFIG_TOML}\n[usage_database]\nurl = \"postgres://db.test/usage\"\n");
    let client = Arc::new(RecordingClient::default());
    let shared = shared_state_with_config_file(client, &with_non_reloadable_changes);

    reload::reload(&shared).unwrap();

    let response = send_chat_with_key(
        build_router_with_shared(shared),
        "gw-secret-v2",
        "public-chat",
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

    let response = send_chat_with_key(
        build_router_with_shared(shared),
        "gw-secret-v2",
        "public-chat",
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
