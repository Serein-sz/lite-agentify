use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{
        Request as HttpRequest, Response, StatusCode,
        header::{CONTENT_TYPE, COOKIE, SET_COOKIE},
    },
};
use chrono::{TimeZone, Utc};
use rust_decimal::Decimal;
use tower::ServiceExt;

use super::{AdminState, LOCKOUT_WINDOW, SESSION_COOKIE, SESSION_TTL, admin_router_with_state, password};
use crate::{
    account::{MemoryAccountStore, Role},
    catalog::{CatalogStore, DeploymentConfig, MemoryCatalogStore, ModelConfig},
    config::GatewayConfig,
    model::Protocol,
    proxy::upstream::{UpstreamClient, UpstreamFuture, UpstreamRequest, UpstreamResponse},
    quota::SpendCounter as _,
    reload::{self, SharedGatewayState},
    state::GatewayState,
    usage::{MemoryUsageRecorder, NoopUsageRecorder, UsageRecord, UsageRecorder, UsageSource},
};

const ADMIN_PASSWORD: &str = "hunter2-test-password";
const ADMIN_USERNAME: &str = "admin";

/// An in-memory account store seeded with the bootstrap admin, matching what
/// the database bootstrap produces in production.
fn admin_store() -> Arc<MemoryAccountStore> {
    Arc::new(MemoryAccountStore::with_user(
        ADMIN_USERNAME,
        ADMIN_PASSWORD,
        Role::Admin,
    ))
}

/// An enabled catalog model with the given `(provider, upstream_model)` chain.
fn catalog_model(name: &str, deployments: &[(&str, &str)]) -> ModelConfig {
    ModelConfig {
        name: name.to_owned(),
        enabled: true,
        created_at: Utc::now(),
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

/// The default catalog for the harness: two enabled OpenAI models over the
/// `openai` provider from `config_toml`, no pricing.
fn default_models() -> Vec<ModelConfig> {
    vec![
        catalog_model("gpt-test", &[("openai", "gpt-test")]),
        catalog_model("gpt-mini", &[("openai", "gpt-mini-real")]),
    ]
}

/// The in-memory catalog matching `config_toml`'s providers plus the given
/// models, so snapshot rebuilds resolve deployments.
fn catalog_store_from(config: &GatewayConfig, models: Vec<ModelConfig>) -> Arc<MemoryCatalogStore> {
    Arc::new(MemoryCatalogStore::with_models(
        config.providers.clone(),
        config.pricing.clone(),
        models,
    ))
}

#[derive(Default)]
struct CountingClient {
    calls: AtomicUsize,
}

impl CountingClient {
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl UpstreamClient for CountingClient {
    fn send(&self, _request: UpstreamRequest) -> UpstreamFuture {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {
            Ok(UpstreamResponse {
                status: StatusCode::OK,
                headers: Default::default(),
                body: Body::from("{}"),
            })
        })
    }
}

/// Config file text. The admin password is stored as plaintext: the fast
/// verify path in tests, and itself a supported configuration (write-back only
/// happens at boot). Routes/providers remain as legacy sections the catalog
/// migration would consume; the live snapshot ignores them.
fn config_toml(admin_password_line: &str) -> String {
    format!(
        r#"# top comment preserved
listen_addr = "127.0.0.1:3000"
gateway_keys = ["gw-secret-key-1"]
{admin_password_line}

[usage_database]
enabled = false
url = "postgres://user:dbpass@localhost/usage"

[[providers]]
id = "openai"
protocol = "openai"
base_url = "http://openai.test"
api_key = "openai-secret-key" # keep my key comment
"#
    )
}

fn write_temp_config(contents: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "lite-agentify-admin-test-{}.toml",
        uuid::Uuid::new_v4()
    ));
    std::fs::write(&path, contents).unwrap();
    path
}

struct Harness {
    app: Router,
    shared: SharedGatewayState,
    client: Arc<CountingClient>,
    accounts: Arc<MemoryAccountStore>,
}

fn harness_with_recorder(recorder: Arc<dyn UsageRecorder>) -> Harness {
    let toml_text = config_toml(&format!("admin_password = \"{ADMIN_PASSWORD}\""));
    let config_path = write_temp_config(&toml_text);
    let config: GatewayConfig = toml::from_str(&toml_text).unwrap();
    let client = Arc::new(CountingClient::default());
    let catalog_store = catalog_store_from(&config, default_models());
    let catalog_snapshot = futures_now(catalog_store.snapshot()).unwrap();
    let state = GatewayState::from_parts(
        config.clone(),
        catalog_snapshot.clone(),
        client.clone(),
        recorder,
    )
    .unwrap();
    let shared = SharedGatewayState::new(state, &config, config_path, catalog_snapshot);
    let accounts = admin_store();
    let app = crate::proxy::router::build_router_with_shared(
        shared.clone(),
        accounts.clone(),
        catalog_store,
        Arc::new(crate::quota::MemoryQuotaStore::default()),
        Arc::new(super::MemorySessionStore::default()),
        None,
    );
    Harness {
        app,
        shared,
        client,
        accounts,
    }
}

/// Blocks on a future that is actually ready immediately (MemoryCatalogStore
/// never awaits), keeping harness construction synchronous.
fn futures_now<F: std::future::Future>(future: F) -> F::Output {
    futures_util::FutureExt::now_or_never(future).expect("memory store futures resolve instantly")
}

fn harness() -> Harness {
    harness_with_recorder(Arc::new(NoopUsageRecorder))
}

async fn request(
    app: &Router,
    method: &str,
    path: &str,
    cookie: Option<&str>,
    json_body: Option<String>,
) -> Response<Body> {
    let mut builder = HttpRequest::builder().method(method).uri(path);
    if let Some(cookie) = cookie {
        builder = builder.header(COOKIE, format!("{SESSION_COOKIE}={cookie}"));
    }
    let body = match json_body {
        Some(json) => {
            builder = builder.header(CONTENT_TYPE, "application/json");
            Body::from(json)
        }
        None => Body::empty(),
    };
    app.clone()
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap()
}

async fn login(app: &Router, password: &str) -> Response<Body> {
    request(
        app,
        "POST",
        "/admin/api/login",
        None,
        Some(format!(
            r#"{{"username":"{ADMIN_USERNAME}","password":"{password}"}}"#
        )),
    )
    .await
}

fn session_token(response: &Response<Body>) -> String {
    let cookie = response
        .headers()
        .get(SET_COOKIE)
        .expect("login response has Set-Cookie")
        .to_str()
        .unwrap();
    cookie
        .split(';')
        .next()
        .unwrap()
        .strip_prefix(&format!("{SESSION_COOKIE}="))
        .expect("cookie name")
        .to_owned()
}

async fn login_token(app: &Router) -> String {
    let response = login(app, ADMIN_PASSWORD).await;
    assert_eq!(response.status(), StatusCode::OK);
    session_token(&response)
}

/// Logs in as a non-admin user (creating it first via the store).
async fn user_token(harness: &Harness, username: &str, password: &str) -> String {
    harness.accounts.insert_user(username, password, Role::User);
    let response = request(
        &harness.app,
        "POST",
        "/admin/api/login",
        None,
        Some(format!(
            r#"{{"username":"{username}","password":"{password}"}}"#
        )),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    session_token(&response)
}

async fn body_json(response: Response<Body>) -> serde_json::Value {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

// --- password bootstrap (first-boot hash write-back) ---

#[test]
fn bootstrap_hashes_plaintext_and_preserves_comments() {
    let toml_text = config_toml("admin_password = \"plain-secret\" # my password comment");
    let config_path = write_temp_config(&toml_text);
    let mut config: GatewayConfig = toml::from_str(&toml_text).unwrap();

    password::bootstrap_admin_password(&mut config, &config_path);

    let stored = config.admin_password.as_deref().unwrap();
    assert!(stored.starts_with("$argon2id$"));
    assert!(password::verify_password(stored, "plain-secret"));

    let rewritten = std::fs::read_to_string(&config_path).unwrap();
    assert!(rewritten.contains("$argon2id$"));
    assert!(!rewritten.contains("plain-secret"));
    assert!(rewritten.contains("# top comment preserved"));
    assert!(rewritten.contains("# keep my key comment"));
    // The same-line comment on the replaced value survives too.
    assert!(rewritten.contains("# my password comment"));

    // The rewritten file parses and boots the same gateway config.
    let reparsed: GatewayConfig = toml::from_str(&rewritten).unwrap();
    assert_eq!(reparsed.admin_password.as_deref(), Some(stored));
}

#[test]
fn bootstrap_leaves_already_hashed_value_untouched() {
    let hash = password::hash_password("some-password").unwrap();
    let toml_text = config_toml(&format!("admin_password = \"{hash}\""));
    let config_path = write_temp_config(&toml_text);
    let before = std::fs::read(&config_path).unwrap();
    let mut config: GatewayConfig = toml::from_str(&toml_text).unwrap();

    password::bootstrap_admin_password(&mut config, &config_path);

    assert_eq!(std::fs::read(&config_path).unwrap(), before);
    assert_eq!(config.admin_password.as_deref(), Some(hash.as_str()));
}

#[test]
fn bootstrap_write_failure_still_provides_in_memory_hash() {
    // A directory path cannot be read as a config file, so write-back fails.
    let mut config: GatewayConfig =
        toml::from_str(&config_toml("admin_password = \"plain-secret\"")).unwrap();

    password::bootstrap_admin_password(&mut config, &std::env::temp_dir());

    let stored = config.admin_password.as_deref().unwrap();
    assert!(stored.starts_with("$argon2id$"));
    assert!(password::verify_password(stored, "plain-secret"));
}

// --- login, sessions, lockout ---

#[tokio::test]
async fn login_success_sets_hardened_cookie() {
    let harness = harness();
    let response = login(&harness.app, ADMIN_PASSWORD).await;

    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response.headers().get(SET_COOKIE).unwrap().to_str().unwrap();
    assert!(cookie.contains("HttpOnly"));
    assert!(cookie.contains("SameSite=Strict"));
    assert!(cookie.contains("Path=/admin"));
}

#[tokio::test]
async fn login_failure_returns_401_without_cookie() {
    let harness = harness();
    let response = login(&harness.app, "wrong-password").await;

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(response.headers().get(SET_COOKIE).is_none());
}

#[tokio::test]
async fn admin_api_requires_a_valid_session() {
    let harness = harness();

    let response = request(&harness.app, "GET", "/admin/api/me", None, None).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let response = request(
        &harness.app,
        "GET",
        "/admin/api/me",
        Some("forged-token"),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn valid_session_grants_api_access() {
    let harness = harness();
    let token = login_token(&harness.app).await;

    let response = request(&harness.app, "GET", "/admin/api/me", Some(&token), None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["role"], "admin");
}

#[tokio::test]
async fn expired_session_is_rejected() {
    let harness = harness();
    // Zero TTL: every session is already expired when first validated.
    let state = AdminState::with_timing(
        harness.shared.clone(),
        admin_store(),
        Arc::new(MemoryCatalogStore::default()),
        Arc::new(crate::quota::MemoryQuotaStore::default()),
        Duration::ZERO,
        LOCKOUT_WINDOW,
    );
    let app = admin_router_with_state(state);

    let response = request(
        &app,
        "POST",
        "/api/login",
        None,
        Some(format!(
            r#"{{"username":"{ADMIN_USERNAME}","password":"{ADMIN_PASSWORD}"}}"#
        )),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let token = session_token(&response);

    let response = request(&app, "GET", "/api/me", Some(&token), None).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn lockout_rejects_even_the_correct_password() {
    let harness = harness();

    for _ in 0..5 {
        let response = login(&harness.app, "wrong-password").await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    let response = login(&harness.app, ADMIN_PASSWORD).await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn expired_lockout_accepts_correct_password_again() {
    let harness = harness();
    // Zero lockout window: the lock expires immediately after it is set.
    let state = AdminState::with_timing(
        harness.shared.clone(),
        admin_store(),
        Arc::new(MemoryCatalogStore::default()),
        Arc::new(crate::quota::MemoryQuotaStore::default()),
        SESSION_TTL,
        Duration::ZERO,
    );
    let app = admin_router_with_state(state);

    for _ in 0..5 {
        let response = request(
            &app,
            "POST",
            "/api/login",
            None,
            Some(format!(
                r#"{{"username":"{ADMIN_USERNAME}","password":"wrong-password"}}"#
            )),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    let response = request(
        &app,
        "POST",
        "/api/login",
        None,
        Some(format!(
            r#"{{"username":"{ADMIN_USERNAME}","password":"{ADMIN_PASSWORD}"}}"#
        )),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn logout_invalidates_the_session() {
    let harness = harness();
    let token = login_token(&harness.app).await;

    let response = request(&harness.app, "POST", "/admin/api/logout", Some(&token), None).await;
    assert_eq!(response.status(), StatusCode::OK);

    let response = request(&harness.app, "GET", "/admin/api/me", Some(&token), None).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

/// A session backend that is down: every read/write errors (what a Redis
/// outage looks like). Auth must fail closed — never open.
struct OutageSessionStore;

#[async_trait::async_trait]
impl super::SessionStore for OutageSessionStore {
    async fn open(
        &self,
        _token: &str,
        _identity: &super::SessionIdentity,
        _ttl: Duration,
    ) -> anyhow::Result<()> {
        anyhow::bail!("session backend down")
    }

    async fn get(&self, _token: &str) -> anyhow::Result<Option<super::SessionIdentity>> {
        anyhow::bail!("session backend down")
    }

    async fn remove(&self, _token: &str) {}

    async fn remove_user(&self, _user_id: uuid::Uuid) -> anyhow::Result<()> {
        anyhow::bail!("session backend down")
    }

    async fn is_locked(&self, _username: &str) -> bool {
        false
    }

    async fn register_failure(&self, _username: &str) {}

    async fn clear_failures(&self, _username: &str) {}
}

#[tokio::test]
async fn session_backend_outage_fails_closed() {
    let harness = harness();
    let state = AdminState::new(
        harness.shared.clone(),
        admin_store(),
        Arc::new(MemoryCatalogStore::default()),
        Arc::new(crate::quota::MemoryQuotaStore::default()),
        Arc::new(OutageSessionStore),
        None,
    );
    let app = admin_router_with_state(state);

    // A presented cookie reads as signed out, never signed in.
    let response = request(&app, "GET", "/api/me", Some("any-token"), None).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    // Correct credentials cannot mint a session either: nothing was stored,
    // so no cookie is issued.
    let response = request(
        &app,
        "POST",
        "/api/login",
        None,
        Some(format!(
            r#"{{"username":"{ADMIN_USERNAME}","password":"{ADMIN_PASSWORD}"}}"#
        )),
    )
    .await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert!(response.headers().get(SET_COOKIE).is_none());
}

#[tokio::test]
async fn session_survives_config_reload() {
    let harness = harness();
    let token = login_token(&harness.app).await;

    reload::reload(&harness.shared).unwrap();

    let response = request(&harness.app, "GET", "/admin/api/me", Some(&token), None).await;
    assert_eq!(response.status(), StatusCode::OK);
}

// --- model catalog API ---

/// Creates a wildcard pricing rule so the pricing gate passes.
async fn create_wildcard_pricing(harness: &Harness, token: &str) {
    let body = serde_json::json!({
        "provider": "*",
        "model": "*",
        "input_per_1m": "1.00",
        "output_per_1m": "2.00",
        "currency": "USD",
    })
    .to_string();
    let response = request(
        &harness.app,
        "POST",
        "/admin/api/pricing",
        Some(token),
        Some(body),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn models_crud_requires_admin() {
    let harness = harness();
    let token = user_token(&harness, "alice", "alice-password-1").await;

    let response = request(&harness.app, "GET", "/admin/api/models", Some(&token), None).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let body = serde_json::json!({ "name": "new-model" }).to_string();
    let response = request(
        &harness.app,
        "POST",
        "/admin/api/models",
        Some(&token),
        Some(body),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    // The names listing is intentionally available to every signed-in user
    // (it feeds the key editor's picker).
    let response = request(
        &harness.app,
        "GET",
        "/admin/api/models/names",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert!(json["models"].as_array().unwrap().iter().any(|name| name == "gpt-test"));
}

#[tokio::test]
async fn create_model_disabled_needs_no_pricing() {
    let harness = harness();
    let token = login_token(&harness.app).await;

    let body = serde_json::json!({
        "name": "draft-model",
        "deployments": [{ "provider_id": "openai", "upstream_model": "gpt-draft" }],
        "enabled": false,
    })
    .to_string();
    let response = request(
        &harness.app,
        "POST",
        "/admin/api/models",
        Some(&token),
        Some(body),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);

    let response = request(&harness.app, "GET", "/admin/api/models", Some(&token), None).await;
    let json = body_json(response).await;
    let draft = json["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|model| model["name"] == "draft-model")
        .expect("draft model listed");
    assert_eq!(draft["enabled"], false);
    // Coverage info names the unpriced deployment.
    assert_eq!(draft["uncovered"][0], "openai:gpt-draft");
}

#[tokio::test]
async fn enabling_model_without_pricing_is_rejected_409() {
    let harness = harness();
    let token = login_token(&harness.app).await;

    let body = serde_json::json!({
        "name": "unpriced",
        "deployments": [{ "provider_id": "openai", "upstream_model": "gpt-unpriced" }],
        "enabled": true,
    })
    .to_string();
    let response = request(
        &harness.app,
        "POST",
        "/admin/api/models",
        Some(&token),
        Some(body),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let json = body_json(response).await;
    assert!(json["error"].as_str().unwrap().contains("gpt-unpriced"));
}

#[tokio::test]
async fn enabling_model_with_wildcard_pricing_succeeds_and_serves() {
    let harness = harness();
    let token = login_token(&harness.app).await;
    create_wildcard_pricing(&harness, &token).await;

    let body = serde_json::json!({
        "name": "priced-model",
        "deployments": [{ "provider_id": "openai", "upstream_model": "gpt-priced" }],
        "enabled": true,
    })
    .to_string();
    let response = request(
        &harness.app,
        "POST",
        "/admin/api/models",
        Some(&token),
        Some(body),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);

    // The snapshot rebuilt: the new model proxies immediately, no restart.
    let response = request(
        &harness.app,
        "POST",
        "/v1/chat/completions",
        None,
        Some(r#"{"model":"priced-model"}"#.to_owned()),
    )
    .await;
    // Unauthenticated → 401 proves it resolved past the 404 layer... use a key.
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let builder = HttpRequest::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(CONTENT_TYPE, "application/json")
        .header("authorization", "Bearer gw-secret-key-1");
    let response = harness
        .app
        .clone()
        .oneshot(
            builder
                .body(Body::from(r#"{"model":"priced-model"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(harness.client.calls(), 1);
}

#[tokio::test]
async fn enabling_deploymentless_model_is_rejected() {
    let harness = harness();
    let token = login_token(&harness.app).await;
    create_wildcard_pricing(&harness, &token).await;

    let body = serde_json::json!({
        "name": "empty-model",
        "deployments": [],
        "enabled": true,
    })
    .to_string();
    let response = request(
        &harness.app,
        "POST",
        "/admin/api/models",
        Some(&token),
        Some(body),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn duplicate_model_name_conflicts() {
    let harness = harness();
    let token = login_token(&harness.app).await;

    let body = serde_json::json!({ "name": "gpt-test" }).to_string();
    let response = request(
        &harness.app,
        "POST",
        "/admin/api/models",
        Some(&token),
        Some(body),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn update_model_rewrites_chain_and_serves_new_deployment() {
    let harness = harness();
    let token = login_token(&harness.app).await;
    create_wildcard_pricing(&harness, &token).await;

    let body = serde_json::json!({
        "deployments": [{ "provider_id": "openai", "upstream_model": "gpt-rewired" }],
        "enabled": true,
    })
    .to_string();
    let response = request(
        &harness.app,
        "PUT",
        "/admin/api/models/gpt-test",
        Some(&token),
        Some(body),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let response = request(&harness.app, "GET", "/admin/api/models", Some(&token), None).await;
    let json = body_json(response).await;
    let updated = json["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|model| model["name"] == "gpt-test")
        .unwrap();
    assert_eq!(updated["deployments"][0]["upstream_model"], "gpt-rewired");
}

#[tokio::test]
async fn update_unknown_model_is_404() {
    let harness = harness();
    let token = login_token(&harness.app).await;

    let body = serde_json::json!({ "deployments": [], "enabled": false }).to_string();
    let response = request(
        &harness.app,
        "PUT",
        "/admin/api/models/no-such",
        Some(&token),
        Some(body),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn deployment_referencing_unknown_provider_is_400() {
    let harness = harness();
    let token = login_token(&harness.app).await;

    let body = serde_json::json!({
        "deployments": [{ "provider_id": "nope", "upstream_model": "x" }],
        "enabled": false,
    })
    .to_string();
    let response = request(
        &harness.app,
        "PUT",
        "/admin/api/models/gpt-test",
        Some(&token),
        Some(body),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn delete_model_removes_it_from_serving() {
    let harness = harness();
    let token = login_token(&harness.app).await;

    let response = request(
        &harness.app,
        "DELETE",
        "/admin/api/models/gpt-test",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let builder = HttpRequest::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(CONTENT_TYPE, "application/json")
        .header("authorization", "Bearer gw-secret-key-1");
    let response = harness
        .app
        .clone()
        .oneshot(builder.body(Body::from(r#"{"model":"gpt-test"}"#)).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(harness.client.calls(), 0);
}

// --- pricing gate on pricing mutations ---

#[tokio::test]
async fn deleting_pricing_that_strips_enabled_model_coverage_is_409() {
    let harness = harness();
    let token = login_token(&harness.app).await;
    create_wildcard_pricing(&harness, &token).await;

    // Enable gpt-test under the wildcard rule.
    let body = serde_json::json!({
        "deployments": [{ "provider_id": "openai", "upstream_model": "gpt-test" }],
        "enabled": true,
    })
    .to_string();
    let response = request(
        &harness.app,
        "PUT",
        "/admin/api/models/gpt-test",
        Some(&token),
        Some(body),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    // Find the wildcard rule's id and try to delete it.
    let response = request(&harness.app, "GET", "/admin/api/pricing", Some(&token), None).await;
    let json = body_json(response).await;
    let rule_id = json["pricing"][0]["id"].as_str().unwrap().to_owned();

    let response = request(
        &harness.app,
        "DELETE",
        &format!("/admin/api/pricing/{rule_id}"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let json = body_json(response).await;
    // Both harness models are enabled; the gate names the first one it finds.
    assert!(json["error"].as_str().unwrap().contains("gpt-"));
}

#[tokio::test]
async fn deleting_pricing_used_only_by_disabled_models_is_allowed() {
    let harness = harness();
    let token = login_token(&harness.app).await;
    create_wildcard_pricing(&harness, &token).await;

    // Both harness models stay disabled → the rule is deletable. Disable them.
    for name in ["gpt-test", "gpt-mini"] {
        let body = serde_json::json!({
            "deployments": [{ "provider_id": "openai", "upstream_model": name }],
            "enabled": false,
        })
        .to_string();
        let response = request(
            &harness.app,
            "PUT",
            &format!("/admin/api/models/{name}"),
            Some(&token),
            Some(body),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    let response = request(&harness.app, "GET", "/admin/api/pricing", Some(&token), None).await;
    let json = body_json(response).await;
    let rule_id = json["pricing"][0]["id"].as_str().unwrap().to_owned();

    let response = request(
        &harness.app,
        "DELETE",
        &format!("/admin/api/pricing/{rule_id}"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn narrowing_pricing_update_that_strips_coverage_is_409() {
    let harness = harness();
    let token = login_token(&harness.app).await;
    create_wildcard_pricing(&harness, &token).await;

    // Enable gpt-test under the wildcard.
    let body = serde_json::json!({
        "deployments": [{ "provider_id": "openai", "upstream_model": "gpt-test" }],
        "enabled": true,
    })
    .to_string();
    request(
        &harness.app,
        "PUT",
        "/admin/api/models/gpt-test",
        Some(&token),
        Some(body),
    )
    .await;

    // Narrow the wildcard to a different provider/model pair.
    let response = request(&harness.app, "GET", "/admin/api/pricing", Some(&token), None).await;
    let json = body_json(response).await;
    let rule_id = json["pricing"][0]["id"].as_str().unwrap().to_owned();

    let body = serde_json::json!({
        "provider": "someone-else",
        "model": "other",
        "input_per_1m": "1.00",
        "output_per_1m": "2.00",
        "currency": "USD",
    })
    .to_string();
    let response = request(
        &harness.app,
        "PUT",
        &format!("/admin/api/pricing/{rule_id}"),
        Some(&token),
        Some(body),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
}

// --- provider delete protection ---

#[tokio::test]
async fn deleting_provider_used_by_a_deployment_is_409() {
    let harness = harness();
    let token = login_token(&harness.app).await;

    let response = request(
        &harness.app,
        "DELETE",
        "/admin/api/providers/openai",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let json = body_json(response).await;
    // The error names a model that still uses the provider.
    assert!(json["error"].as_str().unwrap().contains("gpt-"));
}

#[tokio::test]
async fn deleting_unused_provider_succeeds() {
    let harness = harness();
    let token = login_token(&harness.app).await;

    // Create a provider no deployment references.
    let body = serde_json::json!({
        "id": "spare",
        "protocol": "openai",
        "base_url": "http://spare.test",
        "api_key": "spare-secret",
    })
    .to_string();
    let response = request(
        &harness.app,
        "POST",
        "/admin/api/providers",
        Some(&token),
        Some(body),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);

    let response = request(
        &harness.app,
        "DELETE",
        "/admin/api/providers/spare",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
}

// --- key allowed-models ---

/// Grants `amount` USD to the logged-in session's user via the credits API,
/// so keys created in tests can pass the prepaid quota gate.
async fn grant_self_credit(harness: &Harness, token: &str, amount: &str) {
    let me = body_json(request(&harness.app, "GET", "/admin/api/me", Some(token), None).await).await;
    let user_id = me["user_id"].as_str().unwrap().to_owned();
    let body = serde_json::json!({
        "user_id": user_id,
        "amount_usd": amount,
        "note": "test grant",
    })
    .to_string();
    let response = request(
        &harness.app,
        "POST",
        "/admin/api/credits/grants",
        Some(token),
        Some(body),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn key_with_allowed_models_restricts_proxy_calls() {
    let harness = harness();
    let token = login_token(&harness.app).await;
    grant_self_credit(&harness, &token, "100.00").await;

    // Create a key allowed to call only gpt-test.
    let body = serde_json::json!({
        "name": "restricted",
        "allowed_models": ["gpt-test"],
    })
    .to_string();
    let response = request(
        &harness.app,
        "POST",
        "/admin/api/keys",
        Some(&token),
        Some(body),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    let plaintext = json["key"].as_str().unwrap().to_owned();
    assert_eq!(json["record"]["allowed_models"][0], "gpt-test");

    // Allowed model proxies; the other enabled model is 403.
    let send = |model: &'static str, key: String| {
        let app = harness.app.clone();
        async move {
            app.oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header(CONTENT_TYPE, "application/json")
                    .header("authorization", format!("Bearer {key}"))
                    .body(Body::from(format!(r#"{{"model":"{model}"}}"#)))
                    .unwrap(),
            )
            .await
            .unwrap()
        }
    };
    let response = send("gpt-test", plaintext.clone()).await;
    assert_eq!(response.status(), StatusCode::OK);
    let response = send("gpt-mini", plaintext).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn update_key_allowed_models_takes_effect() {
    let harness = harness();
    let token = login_token(&harness.app).await;
    grant_self_credit(&harness, &token, "100.00").await;

    let body = serde_json::json!({ "name": "editable" }).to_string();
    let response = request(
        &harness.app,
        "POST",
        "/admin/api/keys",
        Some(&token),
        Some(body),
    )
    .await;
    let json = body_json(response).await;
    let plaintext = json["key"].as_str().unwrap().to_owned();
    let key_id = json["record"]["id"].as_str().unwrap().to_owned();

    // Restrict it to gpt-mini only.
    let body = serde_json::json!({ "allowed_models": ["gpt-mini"] }).to_string();
    let response = request(
        &harness.app,
        "PUT",
        &format!("/admin/api/keys/{key_id}"),
        Some(&token),
        Some(body),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let response = harness
        .app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header(CONTENT_TYPE, "application/json")
                .header("authorization", format!("Bearer {plaintext}"))
                .body(Body::from(r#"{"model":"gpt-test"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn empty_allowed_models_list_is_rejected() {
    let harness = harness();
    let token = login_token(&harness.app).await;

    let body = serde_json::json!({ "name": "broken", "allowed_models": [] }).to_string();
    let response = request(
        &harness.app,
        "POST",
        "/admin/api/keys",
        Some(&token),
        Some(body),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

// --- credit quota ---

/// Proxies one request with the given key; the harness upstream always
/// answers 200 with an empty body (no usage → zero cost).
async fn proxy_with_key(harness: &Harness, key: &str, model: &str) -> Response<Body> {
    harness
        .app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header(CONTENT_TYPE, "application/json")
                .header("authorization", format!("Bearer {key}"))
                .body(Body::from(format!(r#"{{"model":"{model}"}}"#)))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn zero_balance_user_gets_402_until_granted() {
    let harness = harness();
    let token = login_token(&harness.app).await;

    // A fresh API-created key belongs to the admin, who has no grants yet.
    let body = serde_json::json!({ "name": "wallet-test" }).to_string();
    let response = request(&harness.app, "POST", "/admin/api/keys", Some(&token), Some(body)).await;
    let plaintext = body_json(response).await["key"].as_str().unwrap().to_owned();

    let response = proxy_with_key(&harness, &plaintext, "gpt-test").await;
    assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);
    let json = body_json(response).await;
    // Protocol-native OpenAI error shape naming the quota problem.
    assert_eq!(json["error"]["code"], "insufficient_quota");
    assert_eq!(harness.client.calls(), 0);

    // Granting credit unblocks the same key immediately.
    grant_self_credit(&harness, &token, "5.00").await;
    let response = proxy_with_key(&harness, &plaintext, "gpt-test").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(harness.client.calls(), 1);
}

#[tokio::test]
async fn negative_grant_correction_can_exhaust_balance() {
    let harness = harness();
    let token = login_token(&harness.app).await;
    grant_self_credit(&harness, &token, "5.00").await;

    let body = serde_json::json!({ "name": "correction-test" }).to_string();
    let response = request(&harness.app, "POST", "/admin/api/keys", Some(&token), Some(body)).await;
    let plaintext = body_json(response).await["key"].as_str().unwrap().to_owned();

    let response = proxy_with_key(&harness, &plaintext, "gpt-test").await;
    assert_eq!(response.status(), StatusCode::OK);

    // A correcting negative grant brings the balance back to zero → 402.
    grant_self_credit(&harness, &token, "-5.00").await;
    let response = proxy_with_key(&harness, &plaintext, "gpt-test").await;
    assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);
}

#[tokio::test]
async fn key_spend_cap_is_independent_of_user_balance() {
    let harness = harness();
    let token = login_token(&harness.app).await;
    grant_self_credit(&harness, &token, "100.00").await;

    // Cap the key at 1 USD, then pre-load its counter to the cap.
    let body = serde_json::json!({ "name": "capped", "spend_cap_usd": "1.00" }).to_string();
    let response = request(&harness.app, "POST", "/admin/api/keys", Some(&token), Some(body)).await;
    let json = body_json(response).await;
    let plaintext = json["key"].as_str().unwrap().to_owned();
    let key_id: uuid::Uuid = json["record"]["id"].as_str().unwrap().parse().unwrap();

    let snapshot = harness.shared.load();
    snapshot
        .spend_counter
        .add(crate::quota::Scope::Key(key_id), rust_decimal::Decimal::ONE)
        .await;

    // The user has plenty of balance, but the key's own cap is reached.
    let response = proxy_with_key(&harness, &plaintext, "gpt-test").await;
    assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);
    let json = body_json(response).await;
    assert!(json["error"]["message"].as_str().unwrap().contains("spend cap"));

    // Raising the cap unblocks it.
    let body = serde_json::json!({ "spend_cap_usd": "10.00", "allowed_models": null }).to_string();
    let response = request(
        &harness.app,
        "PUT",
        &format!("/admin/api/keys/{key_id}"),
        Some(&token),
        Some(body),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let response = proxy_with_key(&harness, &plaintext, "gpt-test").await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn spent_counter_advances_from_usage_cost() {
    // A priced model + usage-bearing upstream response must advance the user
    // counter, eventually tripping the 402 once the grant is consumed.
    let harness = harness();
    let token = login_token(&harness.app).await;
    grant_self_credit(&harness, &token, "0.01").await;

    // Wildcard pricing so gpt-test carries cost: 1M input tokens = 1 USD.
    let body = serde_json::json!({
        "provider": "*",
        "model": "*",
        "input_per_1m": "1.00",
        "output_per_1m": "1.00",
        "currency": "USD",
    })
    .to_string();
    let response = request(&harness.app, "POST", "/admin/api/pricing", Some(&token), Some(body)).await;
    assert_eq!(response.status(), StatusCode::CREATED);

    let body = serde_json::json!({ "name": "spender" }).to_string();
    let response = request(&harness.app, "POST", "/admin/api/keys", Some(&token), Some(body)).await;
    let json = body_json(response).await;
    let plaintext = json["key"].as_str().unwrap().to_owned();
    let key_id: uuid::Uuid = json["record"]["id"].as_str().unwrap().parse().unwrap();
    let me = body_json(request(&harness.app, "GET", "/admin/api/me", Some(&token), None).await).await;
    let user_id: uuid::Uuid = me["user_id"].as_str().unwrap().parse().unwrap();

    // Simulate the cost the usage observer would have added (the harness
    // upstream returns no usage payload): 20k input tokens at 1 USD/1M = 0.02.
    let snapshot = harness.shared.load();
    snapshot
        .spend_counter
        .add(
            crate::quota::Scope::User(user_id),
            rust_decimal::Decimal::new(2, 2),
        )
        .await;
    snapshot
        .spend_counter
        .add(
            crate::quota::Scope::Key(key_id),
            rust_decimal::Decimal::new(2, 2),
        )
        .await;

    // Spent (0.02) ≥ granted (0.01) → 402.
    let response = proxy_with_key(&harness, &plaintext, "gpt-test").await;
    assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);

    // Key spent-to-date is exposed in the listing.
    let response = request(&harness.app, "GET", "/admin/api/keys", Some(&token), None).await;
    let json = body_json(response).await;
    let row = json["keys"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "spender")
        .unwrap();
    assert_eq!(row["spent_usd"], "0.02");
}

#[tokio::test]
async fn balances_and_ledger_are_reported() {
    let harness = harness();
    let token = login_token(&harness.app).await;
    grant_self_credit(&harness, &token, "10.00").await;
    grant_self_credit(&harness, &token, "-2.50").await;

    let response = request(&harness.app, "GET", "/admin/api/me/balance", Some(&token), None).await;
    let json = body_json(response).await;
    assert_eq!(json["granted"], "7.50");
    assert_eq!(json["balance"], "7.50");

    let response = request(&harness.app, "GET", "/admin/api/credits", Some(&token), None).await;
    let json = body_json(response).await;
    let row = json["balances"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["username"] == ADMIN_USERNAME)
        .unwrap();
    assert_eq!(row["granted"], "7.50");

    let response = request(
        &harness.app,
        "GET",
        "/admin/api/credits/ledger",
        Some(&token),
        None,
    )
    .await;
    let json = body_json(response).await;
    let grants = json["grants"].as_array().unwrap();
    assert_eq!(grants.len(), 2);
    // Newest first; both rows carry the grantor's username.
    assert_eq!(grants[0]["amount_usd"], "-2.50");
    assert_eq!(grants[0]["granted_by"], ADMIN_USERNAME);
}

#[tokio::test]
async fn credits_apis_require_admin_except_own_balance() {
    let harness = harness();
    let alice = user_token(&harness, "alice", "alice-password-1").await;

    let response = request(&harness.app, "GET", "/admin/api/credits", Some(&alice), None).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let body = serde_json::json!({
        "user_id": uuid::Uuid::new_v4(),
        "amount_usd": "1.00",
    })
    .to_string();
    let response = request(
        &harness.app,
        "POST",
        "/admin/api/credits/grants",
        Some(&alice),
        Some(body),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    // Own balance is visible to every role.
    let response = request(&harness.app, "GET", "/admin/api/me/balance", Some(&alice), None).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_json(response).await["balance"], "0");
}

#[tokio::test]
async fn reconciliation_resets_counters_to_store_truth() {
    // Drift the counter, then reconcile against the (memory) quota store's
    // sums — the counter snaps back to truth.
    let store = crate::quota::MemoryQuotaStore::default();
    let counter = crate::quota::MemoryCounter::default();
    let user_id = uuid::Uuid::new_v4();
    let mut sums = crate::quota::SpendSums::default();
    sums.by_user.insert(user_id, rust_decimal::Decimal::new(125, 2));
    store.set_spend_sums(sums);

    counter
        .add(crate::quota::Scope::User(user_id), rust_decimal::Decimal::from(999))
        .await;
    crate::quota::reconcile_counters(&store, &counter).await.unwrap();
    assert_eq!(
        counter.get(crate::quota::Scope::User(user_id)).await,
        rust_decimal::Decimal::new(125, 2)
    );
}

#[tokio::test]
async fn non_owner_cannot_edit_another_users_key() {
    let harness = harness();
    let admin_token = login_token(&harness.app).await;

    // Admin creates a key (owned by admin).
    let body = serde_json::json!({ "name": "admins-key" }).to_string();
    let response = request(
        &harness.app,
        "POST",
        "/admin/api/keys",
        Some(&admin_token),
        Some(body),
    )
    .await;
    let key_id = body_json(response).await["record"]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    // A non-admin user cannot edit it — 404, not 403, to avoid id probing.
    let alice = user_token(&harness, "alice", "alice-password-1").await;
    let body = serde_json::json!({ "allowed_models": ["gpt-test"] }).to_string();
    let response = request(
        &harness.app,
        "PUT",
        &format!("/admin/api/keys/{key_id}"),
        Some(&alice),
        Some(body),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

// --- usage query API ---

fn record(
    request_id: &str,
    provider: &str,
    model: &str,
    status: u16,
    minute: u32,
    cost_cents: i64,
) -> UsageRecord {
    UsageRecord {
        request_id: request_id.to_owned(),
        created_at: Utc.with_ymd_and_hms(2026, 7, 1, 10, minute, 0).unwrap(),
        provider_id: provider.to_owned(),
        protocol: Protocol::OpenAi,
        path: "/v1/chat/completions".to_owned(),
        user_id: None,
        api_key_id: None,
        requested_model: Some(model.to_owned()),
        upstream_model: Some(model.to_owned()),
        status,
        latency_ms: 100,
        input_tokens: Some(100),
        output_tokens: Some(20),
        cached_input_tokens: None,
        cache_read_tokens: None,
        cache_write_tokens: None,
        total_tokens: Some(120),
        estimated_cost: Some(Decimal::new(cost_cents, 2)),
        currency: Some("USD".to_owned()),
        usage_source: UsageSource::ProviderResponse,
        pricing_source: None,
    }
}

async fn seeded_usage_harness() -> Harness {
    let recorder = Arc::new(MemoryUsageRecorder::default());
    let harness = harness_with_recorder(recorder.clone());
    let records = [
        record("r1", "openai", "gpt-real", 200, 1, 100),
        record("r2", "openai", "gpt-real", 200, 2, 100),
        record("r3", "openai", "gpt-real", 500, 3, 0),
        record("r4", "openai", "gpt-mini", 200, 4, 50),
        record("r5", "openai", "gpt-mini", 404, 5, 0),
        record("r6", "anthropic", "claude", 200, 6, 200),
        record("r7", "anthropic", "claude", 500, 7, 0),
    ];
    for entry in records {
        recorder.record(entry).await.unwrap();
    }
    harness
}

#[tokio::test]
async fn usage_endpoints_report_disabled_without_a_database() {
    let harness = harness(); // NoopUsageRecorder: no readable store
    let token = login_token(&harness.app).await;

    let response = request(&harness.app, "GET", "/admin/api/usage", Some(&token), None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["usage_enabled"], false);
    assert_eq!(json["total"], 0);

    let response = request(
        &harness.app,
        "GET",
        "/admin/api/usage/summary",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["usage_enabled"], false);
    assert_eq!(json["totals"]["requests"], 0);
}

#[tokio::test]
async fn usage_list_paginates_newest_first() {
    let harness = seeded_usage_harness().await;
    let token = login_token(&harness.app).await;

    let response = request(
        &harness.app,
        "GET",
        "/admin/api/usage?page=2&page_size=3",
        Some(&token),
        None,
    )
    .await;
    let json = body_json(response).await;
    assert_eq!(json["usage_enabled"], true);
    assert_eq!(json["total"], 7);
    let rows = json["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    // Newest first: page 2 of size 3 starts at the 4th newest (r4).
    assert_eq!(rows[0]["request_id"], "r4");
}

#[tokio::test]
async fn usage_list_applies_filters() {
    let harness = seeded_usage_harness().await;
    let token = login_token(&harness.app).await;

    let response = request(
        &harness.app,
        "GET",
        "/admin/api/usage?provider=openai",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(body_json(response).await["total"], 5);

    let response = request(
        &harness.app,
        "GET",
        "/admin/api/usage?status=5xx",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(body_json(response).await["total"], 2);

    let response = request(
        &harness.app,
        "GET",
        "/admin/api/usage?status=404",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(body_json(response).await["total"], 1);

    let response = request(
        &harness.app,
        "GET",
        "/admin/api/usage?model=claude",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(body_json(response).await["total"], 2);

    let response = request(
        &harness.app,
        "GET",
        "/admin/api/usage?from=2026-07-01T10:06:00Z",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(body_json(response).await["total"], 2);

    let response = request(
        &harness.app,
        "GET",
        "/admin/api/usage?status=bogus",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn usage_summary_aggregates_totals_series_and_breakdown() {
    let harness = seeded_usage_harness().await;
    let token = login_token(&harness.app).await;

    let response = request(
        &harness.app,
        "GET",
        "/admin/api/usage/summary?bucket=hour",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;

    assert_eq!(json["usage_enabled"], true);
    let totals = &json["totals"];
    assert_eq!(totals["requests"], 7);
    assert_eq!(totals["total_tokens"], 7 * 120);
    // 3 of 7 requests have status >= 400.
    let error_rate = totals["error_rate"].as_f64().unwrap();
    assert!((error_rate - 3.0 / 7.0).abs() < 1e-9);
    assert_eq!(totals["cost"][0]["currency"], "USD");
    assert_eq!(totals["cost"][0]["amount"], "4.50");

    // All records fall into the same hour bucket.
    let series = json["series"].as_array().unwrap();
    assert_eq!(series.len(), 1);
    assert_eq!(series[0]["requests"], 7);

    let breakdown = json["breakdown"].as_array().unwrap();
    assert_eq!(breakdown.len(), 3); // openai×gpt-real, openai×gpt-mini, anthropic×claude
    assert!(
        breakdown
            .iter()
            .any(|row| row["provider_id"] == "anthropic" && row["model"] == "claude")
    );
}

// --- /admin path reservation and asset serving ---

#[tokio::test]
async fn admin_paths_are_never_proxied_when_enabled() {
    let harness = harness();

    let response = request(&harness.app, "GET", "/admin/some/deep/path", None, None).await;
    assert_ne!(response.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(harness.client.calls(), 0);

    let response = request(&harness.app, "GET", "/admin/api/unknown", None, None).await;
    // Unknown API paths 404 from the asset handler's api guard, never the SPA
    // shell and never the proxy.
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(harness.client.calls(), 0);
}

#[tokio::test]
async fn admin_paths_are_served_and_never_proxied() {
    // The console is always enabled now (accounts live in the database, the
    // bootstrap admin always exists). `/admin` paths are gateway-owned and must
    // never fall through to the upstream proxy.
    let harness = harness();

    let response = request(&harness.app, "GET", "/admin", None, None).await;
    // Served by the SPA shell (or asset handler), not proxied.
    assert_ne!(response.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(harness.client.calls(), 0);

    // Login without a session reaches the login handler (a bad body → 4xx),
    // never the upstream proxy.
    let response = request(
        &harness.app,
        "POST",
        "/admin/api/login",
        None,
        Some(r#"{"username":"admin","password":"wrong"}"#.to_owned()),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(harness.client.calls(), 0);
}

#[tokio::test]
async fn non_admin_paths_still_proxy() {
    let harness = harness();

    let response = request(
        &harness.app,
        "POST",
        "/v1/chat/completions",
        None,
        Some(r#"{"model":"gpt-test"}"#.to_owned()),
    )
    .await;
    // Unauthenticated proxy requests are rejected by gateway auth, proving the
    // request reached the proxy handler rather than the admin console.
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let mut builder = HttpRequest::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(CONTENT_TYPE, "application/json");
    builder = builder.header("authorization", "Bearer gw-secret-key-1");
    let response = harness
        .app
        .clone()
        .oneshot(builder.body(Body::from(r#"{"model":"gpt-test"}"#)).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(harness.client.calls(), 1);
}
