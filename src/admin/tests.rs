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
    config::GatewayConfig,
    model::Protocol,
    proxy::{
        router::build_router_with_shared,
        upstream::{UpstreamClient, UpstreamFuture, UpstreamRequest, UpstreamResponse},
    },
    reload::{self, SharedGatewayState},
    state::GatewayState,
    usage::{MemoryUsageRecorder, NoopUsageRecorder, UsageRecord, UsageRecorder, UsageSource},
};

const ADMIN_PASSWORD: &str = "hunter2-test-password";

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

/// Config file text with every kind of secret and comments to preserve.
/// The admin password is stored as plaintext: the fast verify path in tests,
/// and itself a supported configuration (write-back only happens at boot).
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

[[routes]]
path_prefix = "/v1/chat/completions"
providers = ["openai"]
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
    config_path: PathBuf,
    client: Arc<CountingClient>,
}

fn harness_with_recorder(recorder: Arc<dyn UsageRecorder>) -> Harness {
    let toml_text = config_toml(&format!("admin_password = \"{ADMIN_PASSWORD}\""));
    let config_path = write_temp_config(&toml_text);
    let config: GatewayConfig = toml::from_str(&toml_text).unwrap();
    let client = Arc::new(CountingClient::default());
    let state =
        GatewayState::from_config_with_upstream_and_recorder(config.clone(), client.clone(), recorder)
            .unwrap();
    let shared = SharedGatewayState::new(state, &config, config_path.clone());
    let app = build_router_with_shared(shared.clone());
    Harness {
        app,
        shared,
        config_path,
        client,
    }
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
        Some(format!(r#"{{"password":"{password}"}}"#)),
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

async fn body_json(response: Response<Body>) -> serde_json::Value {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

async fn get_config_payload(app: &Router, token: &str) -> (String, String) {
    let response = request(app, "GET", "/admin/api/config", Some(token), None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    (
        json["content"].as_str().unwrap().to_owned(),
        json["hash"].as_str().unwrap().to_owned(),
    )
}

async fn put_config(app: &Router, token: &str, content: &str, base_hash: &str) -> Response<Body> {
    let body = serde_json::json!({ "content": content, "base_hash": base_hash }).to_string();
    request(app, "PUT", "/admin/api/config", Some(token), Some(body)).await
}

async fn put_structured(
    app: &Router,
    token: &str,
    config: serde_json::Value,
    base_hash: &str,
) -> Response<Body> {
    let body = serde_json::json!({ "config": config, "base_hash": base_hash }).to_string();
    request(
        app,
        "PUT",
        "/admin/api/config/structured",
        Some(token),
        Some(body),
    )
    .await
}

/// The structured form state matching `config_toml`, every secret untouched
/// (still the masked sentinel, as the form submits unedited secret fields).
fn base_structured() -> serde_json::Value {
    serde_json::json!({
        "gateway_keys": ["__MASKED__"],
        "providers": [{
            "id": "openai",
            "protocol": "openai",
            "base_url": "http://openai.test",
            "api_key": "__MASKED__",
            "model_aliases": {}
        }],
        "routes": [{
            "path_prefix": "/v1/chat/completions",
            "providers": ["openai"]
        }],
        "pricing": []
    })
}

async fn reveal(app: &Router, token: Option<&str>, field: &str) -> Response<Body> {
    let body = serde_json::json!({ "field": field }).to_string();
    request(app, "POST", "/admin/api/config/reveal", token, Some(body)).await
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

    let response = request(&harness.app, "GET", "/admin/api/config", None, None).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let response = request(
        &harness.app,
        "GET",
        "/admin/api/config",
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

    let response = request(&harness.app, "GET", "/admin/api/config", Some(&token), None).await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn expired_session_is_rejected() {
    let harness = harness();
    // Zero TTL: every session is already expired when first validated.
    let state = AdminState::with_timing(harness.shared.clone(), Duration::ZERO, LOCKOUT_WINDOW);
    let app = admin_router_with_state(state);

    let response = request(
        &app,
        "POST",
        "/api/login",
        None,
        Some(format!(r#"{{"password":"{ADMIN_PASSWORD}"}}"#)),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let token = session_token(&response);

    let response = request(&app, "GET", "/api/config", Some(&token), None).await;
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
    let state = AdminState::with_timing(harness.shared.clone(), SESSION_TTL, Duration::ZERO);
    let app = admin_router_with_state(state);

    for _ in 0..5 {
        let response = request(
            &app,
            "POST",
            "/api/login",
            None,
            Some(r#"{"password":"wrong-password"}"#.to_owned()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    let response = request(
        &app,
        "POST",
        "/api/login",
        None,
        Some(format!(r#"{{"password":"{ADMIN_PASSWORD}"}}"#)),
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

    let response = request(&harness.app, "GET", "/admin/api/config", Some(&token), None).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn session_survives_config_reload() {
    let harness = harness();
    let token = login_token(&harness.app).await;

    reload::reload(&harness.shared).unwrap();

    let response = request(&harness.app, "GET", "/admin/api/config", Some(&token), None).await;
    assert_eq!(response.status(), StatusCode::OK);
}

// --- config management API ---

#[tokio::test]
async fn get_config_masks_every_secret_and_reports_file_hash() {
    let harness = harness();
    let token = login_token(&harness.app).await;
    let (content, hash) = get_config_payload(&harness.app, &token).await;

    assert!(!content.contains("openai-secret-key"));
    assert!(!content.contains("gw-secret-key-1"));
    assert!(!content.contains("dbpass"));
    assert!(!content.contains(ADMIN_PASSWORD));
    assert!(content.contains("__MASKED__"));
    // Comments survive masking.
    assert!(content.contains("# keep my key comment"));

    let disk = std::fs::read(&harness.config_path).unwrap();
    let expected = {
        use sha2::{Digest, Sha256};
        Sha256::digest(&disk)
            .iter()
            .fold(String::new(), |mut out, byte| {
                use std::fmt::Write;
                let _ = write!(out, "{byte:02x}");
                out
            })
    };
    assert_eq!(hash, expected);
}

#[tokio::test]
async fn put_with_untouched_sentinels_preserves_secrets_on_disk() {
    let harness = harness();
    let token = login_token(&harness.app).await;
    let (content, hash) = get_config_payload(&harness.app, &token).await;

    let response = put_config(&harness.app, &token, &content, &hash).await;
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["warnings"].as_array().unwrap().len(), 0);

    let disk = std::fs::read_to_string(&harness.config_path).unwrap();
    assert!(disk.contains("openai-secret-key"));
    assert!(disk.contains("gw-secret-key-1"));
    assert!(disk.contains("postgres://user:dbpass@localhost/usage"));
    assert!(disk.contains(ADMIN_PASSWORD));
    assert!(!disk.contains("__MASKED__"));
}

#[tokio::test]
async fn put_with_replaced_secret_persists_and_activates_it() {
    let harness = harness();
    let token = login_token(&harness.app).await;
    let (content, hash) = get_config_payload(&harness.app, &token).await;

    // "openai-secret-key" is masked as __MASKED__-key (last 4 chars kept).
    let edited = content.replace("__MASKED__-key", "new-upstream-secret");
    let response = put_config(&harness.app, &token, &edited, &hash).await;
    assert_eq!(response.status(), StatusCode::OK);

    let disk = std::fs::read_to_string(&harness.config_path).unwrap();
    assert!(disk.contains("new-upstream-secret"));
    assert!(!disk.contains("openai-secret-key"));
    // The new secret is live without a restart.
    assert_eq!(
        harness.shared.load().providers.get("openai").unwrap().api_key,
        "new-upstream-secret"
    );
}

#[tokio::test]
async fn put_activates_new_routes_without_restart() {
    let harness = harness();
    let token = login_token(&harness.app).await;
    let (content, hash) = get_config_payload(&harness.app, &token).await;

    let mut edited = content.clone();
    edited.push_str("\n[[routes]]\npath_prefix = \"/v1/embeddings\"\nproviders = [\"openai\"]\n");
    let response = put_config(&harness.app, &token, &edited, &hash).await;
    assert_eq!(response.status(), StatusCode::OK);

    assert!(
        harness
            .shared
            .load()
            .match_route("/v1/embeddings", None)
            .is_some()
    );
}

#[tokio::test]
async fn put_invalid_toml_is_rejected_and_file_unchanged() {
    let harness = harness();
    let token = login_token(&harness.app).await;
    let (_, hash) = get_config_payload(&harness.app, &token).await;
    let before = std::fs::read(&harness.config_path).unwrap();

    let response = put_config(&harness.app, &token, "not toml ][", &hash).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(std::fs::read(&harness.config_path).unwrap(), before);
}

#[tokio::test]
async fn put_semantically_invalid_config_is_rejected_and_file_unchanged() {
    let harness = harness();
    let token = login_token(&harness.app).await;
    let (content, hash) = get_config_payload(&harness.app, &token).await;
    let before = std::fs::read(&harness.config_path).unwrap();

    let edited = content.replace("providers = [\"openai\"]", "providers = [\"missing\"]");
    let response = put_config(&harness.app, &token, &edited, &hash).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(std::fs::read(&harness.config_path).unwrap(), before);
}

#[tokio::test]
async fn put_with_stale_hash_conflicts_and_returns_fresh_content() {
    let harness = harness();
    let token = login_token(&harness.app).await;
    let (content, _) = get_config_payload(&harness.app, &token).await;
    let before = std::fs::read(&harness.config_path).unwrap();

    let response = put_config(&harness.app, &token, &content, "stale-hash").await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let json = body_json(response).await;
    assert!(json["content"].as_str().unwrap().contains("__MASKED__"));
    assert!(json["hash"].as_str().is_some());
    assert_eq!(std::fs::read(&harness.config_path).unwrap(), before);
}

#[tokio::test]
async fn put_with_unresolvable_sentinel_names_the_field() {
    let harness = harness();
    let token = login_token(&harness.app).await;
    let (content, hash) = get_config_payload(&harness.app, &token).await;
    let before = std::fs::read(&harness.config_path).unwrap();

    let mut edited = content.clone();
    edited.push_str(
        "\n[[providers]]\nid = \"brand-new\"\nprotocol = \"openai\"\nbase_url = \"http://new.test\"\napi_key = \"__MASKED__\"\n",
    );
    let response = put_config(&harness.app, &token, &edited, &hash).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let json = body_json(response).await;
    assert!(json["error"].as_str().unwrap().contains("brand-new"));
    assert_eq!(std::fs::read(&harness.config_path).unwrap(), before);
}

#[tokio::test]
async fn put_reports_restart_required_warnings() {
    let harness = harness();
    let token = login_token(&harness.app).await;
    let (content, hash) = get_config_payload(&harness.app, &token).await;

    let edited = content.replace("127.0.0.1:3000", "127.0.0.1:4000");
    let response = put_config(&harness.app, &token, &edited, &hash).await;
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    let warnings = json["warnings"].as_array().unwrap();
    assert!(
        warnings
            .iter()
            .any(|warning| warning.as_str().unwrap().contains("listen_addr"))
    );
}

// --- structured config write API ---

#[tokio::test]
async fn structured_field_edit_preserves_comments_and_masked_secrets() {
    let harness = harness();
    let token = login_token(&harness.app).await;
    let (_, hash) = get_config_payload(&harness.app, &token).await;

    let mut config = base_structured();
    config["providers"][0]["base_url"] = "http://openai-new.test".into();
    let response = put_structured(&harness.app, &token, config, &hash).await;
    assert_eq!(response.status(), StatusCode::OK);

    let disk = std::fs::read_to_string(&harness.config_path).unwrap();
    assert!(disk.contains("http://openai-new.test"));
    // Comments on surviving nodes are preserved, including the one on the
    // same line as the (untouched, sentinel-submitted) secret.
    assert!(disk.contains("# top comment preserved"));
    assert!(disk.contains("# keep my key comment"));
    // Masked sentinels round-trip to the on-disk secrets.
    assert!(disk.contains("openai-secret-key"));
    assert!(disk.contains("gw-secret-key-1"));
    assert!(!disk.contains("__MASKED__"));
    // Untouched sections survive the reconcile.
    assert!(disk.contains("postgres://user:dbpass@localhost/usage"));
    assert!(disk.contains(ADMIN_PASSWORD));
}

#[tokio::test]
async fn structured_added_provider_persists_and_activates() {
    let harness = harness();
    let token = login_token(&harness.app).await;
    let (_, hash) = get_config_payload(&harness.app, &token).await;

    let mut config = base_structured();
    config["providers"].as_array_mut().unwrap().push(serde_json::json!({
        "id": "anthropic",
        "protocol": "anthropic",
        "base_url": "http://anthropic.test",
        "api_key": "anthropic-real-key",
        "anthropic_version": "2023-06-01",
        "model_aliases": { "claude": "claude-real" }
    }));
    let response = put_structured(&harness.app, &token, config, &hash).await;
    assert_eq!(response.status(), StatusCode::OK);

    let disk = std::fs::read_to_string(&harness.config_path).unwrap();
    assert!(disk.contains("anthropic-real-key"));
    assert!(disk.contains("claude-real"));
    // Live without a restart.
    assert!(harness.shared.load().providers.contains_key("anthropic"));
}

#[tokio::test]
async fn structured_removed_entries_are_deleted() {
    let harness = harness();
    let token = login_token(&harness.app).await;
    let (_, hash) = get_config_payload(&harness.app, &token).await;

    // Replace the openai provider with a new one (at least one provider and
    // one route must remain for the config to validate).
    let mut config = base_structured();
    config["providers"] = serde_json::json!([{
        "id": "backup",
        "protocol": "openai",
        "base_url": "http://backup.test",
        "api_key": "backup-real-key",
        "model_aliases": {}
    }]);
    config["routes"][0]["providers"] = serde_json::json!(["backup"]);
    let response = put_structured(&harness.app, &token, config, &hash).await;
    assert_eq!(response.status(), StatusCode::OK);

    let disk = std::fs::read_to_string(&harness.config_path).unwrap();
    assert!(!disk.contains("id = \"openai\""));
    assert!(!disk.contains("openai-secret-key"));
    assert!(disk.contains("id = \"backup\""));
    assert!(!harness.shared.load().providers.contains_key("openai"));
    assert!(harness.shared.load().providers.contains_key("backup"));
}

#[tokio::test]
async fn structured_invalid_config_is_rejected_and_file_unchanged() {
    let harness = harness();
    let token = login_token(&harness.app).await;
    let (_, hash) = get_config_payload(&harness.app, &token).await;
    let before = std::fs::read(&harness.config_path).unwrap();

    let mut config = base_structured();
    config["routes"][0]["providers"] = serde_json::json!(["missing"]);
    let response = put_structured(&harness.app, &token, config, &hash).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(std::fs::read(&harness.config_path).unwrap(), before);
}

#[tokio::test]
async fn structured_stale_hash_conflicts_and_file_unchanged() {
    let harness = harness();
    let token = login_token(&harness.app).await;
    let before = std::fs::read(&harness.config_path).unwrap();

    let response = put_structured(&harness.app, &token, base_structured(), "stale-hash").await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let json = body_json(response).await;
    assert!(json["content"].as_str().unwrap().contains("__MASKED__"));
    assert!(json["hash"].as_str().is_some());
    assert_eq!(std::fs::read(&harness.config_path).unwrap(), before);
}

#[tokio::test]
async fn structured_changed_secret_persists_and_activates() {
    let harness = harness();
    let token = login_token(&harness.app).await;
    let (_, hash) = get_config_payload(&harness.app, &token).await;

    let mut config = base_structured();
    config["providers"][0]["api_key"] = "new-structured-secret".into();
    let response = put_structured(&harness.app, &token, config, &hash).await;
    assert_eq!(response.status(), StatusCode::OK);

    let disk = std::fs::read_to_string(&harness.config_path).unwrap();
    assert!(disk.contains("new-structured-secret"));
    assert!(!disk.contains("openai-secret-key"));
    assert_eq!(
        harness.shared.load().providers.get("openai").unwrap().api_key,
        "new-structured-secret"
    );
}

#[tokio::test]
async fn structured_put_adds_pricing_entry() {
    let harness = harness();
    let token = login_token(&harness.app).await;
    let (_, hash) = get_config_payload(&harness.app, &token).await;

    let mut config = base_structured();
    config["pricing"] = serde_json::json!([{
        "provider": "*",
        "model": "*",
        "input_per_1m": "2.00",
        "output_per_1m": "8.00",
        "cached_input_per_1m": "0.50",
        "currency": "USD"
    }]);
    let response = put_structured(&harness.app, &token, config, &hash).await;
    assert_eq!(response.status(), StatusCode::OK);

    let disk = std::fs::read_to_string(&harness.config_path).unwrap();
    assert!(disk.contains("[[pricing]]"));
    assert!(disk.contains("\"2.00\""));
}

// --- secret reveal API ---

#[tokio::test]
async fn reveal_returns_the_single_requested_secret() {
    let harness = harness();
    let token = login_token(&harness.app).await;

    let response = reveal(&harness.app, Some(&token), "providers.openai.api_key").await;
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["value"], "openai-secret-key");
    // Exactly one field in the response: no other secret rides along.
    assert_eq!(json.as_object().unwrap().len(), 1);

    let response = reveal(&harness.app, Some(&token), "gateway_keys.0").await;
    assert_eq!(body_json(response).await["value"], "gw-secret-key-1");

    let response = reveal(&harness.app, Some(&token), "usage_database.url").await;
    assert_eq!(
        body_json(response).await["value"],
        "postgres://user:dbpass@localhost/usage"
    );
}

#[tokio::test]
async fn reveal_requires_a_session() {
    let harness = harness();

    let response = reveal(&harness.app, None, "providers.openai.api_key").await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn reveal_unknown_reference_is_404() {
    let harness = harness();
    let token = login_token(&harness.app).await;

    let response = reveal(&harness.app, Some(&token), "providers.missing.api_key").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = reveal(&harness.app, Some(&token), "gateway_keys.9").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn reveal_non_secret_reference_is_400() {
    let harness = harness();
    let token = login_token(&harness.app).await;

    for field in ["listen_addr", "providers.openai.base_url", "admin_password"] {
        let response = reveal(&harness.app, Some(&token), field).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "field: {field}");
        let json = body_json(response).await;
        assert!(json.get("value").is_none());
    }
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
async fn admin_paths_return_404_when_disabled_and_are_not_proxied() {
    let toml_text = config_toml("# no admin password");
    let config_path = write_temp_config(&toml_text);
    let config: GatewayConfig = toml::from_str(&toml_text).unwrap();
    let client = Arc::new(CountingClient::default());
    let state = GatewayState::from_config_with_upstream_and_recorder(
        config.clone(),
        client.clone(),
        Arc::new(NoopUsageRecorder),
    )
    .unwrap();
    let shared = SharedGatewayState::new(state, &config, config_path);
    let app = build_router_with_shared(shared);

    let response = request(&app, "GET", "/admin", None, None).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = request(
        &app,
        "POST",
        "/admin/api/login",
        None,
        Some(r#"{"password":"x"}"#.to_owned()),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(client.calls(), 0);
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
