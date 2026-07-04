use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use axum::{
    body::{Body, Bytes, to_bytes},
    http::{
        HeaderMap, HeaderValue, Request as HttpRequest, StatusCode,
        header::{AUTHORIZATION, CONTENT_TYPE},
    },
};
use tower::ServiceExt;

use super::{
    config::{GatewayConfig, ProviderConfig, RouteConfig},
    model::{Protocol, default_listen_addr},
    router::build_router_with_state,
    state::GatewayState,
    upstream::{UpstreamClient, UpstreamFuture, UpstreamRequest, UpstreamResponse},
};

#[derive(Default)]
struct RecordingClient {
    calls: AtomicUsize,
    requests: Mutex<Vec<UpstreamRequest>>,
    response_body: Mutex<Option<&'static str>>,
}

impl RecordingClient {
    fn with_body(body: &'static str) -> Self {
        Self {
            response_body: Mutex::new(Some(body)),
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

        Box::pin(async move {
            let mut headers = HeaderMap::new();
            headers.insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));

            Ok(UpstreamResponse {
                status: StatusCode::OK,
                headers,
                body: Body::from(body),
            })
        })
    }
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
            },
            ProviderConfig {
                id: "anthropic".to_owned(),
                protocol: Protocol::Anthropic,
                base_url: "http://anthropic.test".to_owned(),
                api_key: "anthropic-secret".to_owned(),
                anthropic_version: Some("2023-06-01".to_owned()),
            },
        ],
        routes: vec![
            RouteConfig {
                path_prefix: "/v1/chat/completions".to_owned(),
                provider: "openai".to_owned(),
                model_prefix: None,
            },
            RouteConfig {
                path_prefix: "/v1/messages".to_owned(),
                provider: "anthropic".to_owned(),
                model_prefix: None,
            },
        ],
    }
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

    assert!(state.match_route("/v1/messages", br#"{}"#).is_none());
    assert!(
        state
            .match_route("/v1/chat/completions", br#"{}"#)
            .is_some()
    );
}

#[test]
fn fails_when_no_routes_reference_configured_providers() {
    let mut config = config();
    for route in &mut config.routes {
        route.provider = "missing".to_owned();
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
fn matches_model_prefix_route() {
    let mut config = config();
    config.providers.push(ProviderConfig {
        id: "deepseek".to_owned(),
        protocol: Protocol::OpenAi,
        base_url: "http://deepseek.test".to_owned(),
        api_key: "deepseek-secret".to_owned(),
        anthropic_version: None,
    });
    config.routes.insert(
        0,
        RouteConfig {
            path_prefix: "/v1/chat/completions".to_owned(),
            provider: "deepseek".to_owned(),
            model_prefix: Some("deepseek-".to_owned()),
        },
    );

    let state = GatewayState::from_config(config).unwrap();
    let (_, provider) = state
        .match_route(
            "/v1/chat/completions",
            br#"{"model":"deepseek-chat","messages":[]}"#,
        )
        .unwrap();

    assert_eq!(provider.id, "deepseek");
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
