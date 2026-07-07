mod assets;
mod config_api;
mod password;
mod usage_api;

#[cfg(test)]
mod tests;

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::{
    Json, Router,
    extract::{Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use tracing::warn;

use crate::reload::SharedGatewayState;

pub use password::bootstrap_admin_password;

const SESSION_COOKIE: &str = "lite_agentify_admin";
const SESSION_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const LOCKOUT_THRESHOLD: u32 = 5;
const LOCKOUT_WINDOW: Duration = Duration::from_secs(60);

/// State for the admin console. Sessions and the login limiter live here —
/// beside, not inside, the arc-swapped gateway state — so a config hot reload
/// never invalidates sessions.
#[derive(Clone)]
pub(crate) struct AdminState {
    shared: SharedGatewayState,
    sessions: Arc<Mutex<HashMap<String, Instant>>>,
    limiter: Arc<Mutex<LoginLimiter>>,
    session_ttl: Duration,
}

impl AdminState {
    fn new(shared: SharedGatewayState) -> Self {
        Self::with_timing(shared, SESSION_TTL, LOCKOUT_WINDOW)
    }

    fn with_timing(shared: SharedGatewayState, session_ttl: Duration, lockout: Duration) -> Self {
        Self {
            shared,
            sessions: Arc::new(Mutex::new(HashMap::new())),
            limiter: Arc::new(Mutex::new(LoginLimiter::new(lockout))),
            session_ttl,
        }
    }

    pub(crate) fn shared(&self) -> &SharedGatewayState {
        &self.shared
    }

    fn open_session(&self) -> String {
        let token = random_token();
        let expires_at = Instant::now() + self.session_ttl;
        self.sessions
            .lock()
            .unwrap()
            .insert(token.clone(), expires_at);
        token
    }

    /// Validates a session token, lazily removing it once expired.
    fn session_is_valid(&self, token: &str) -> bool {
        let mut sessions = self.sessions.lock().unwrap();
        match sessions.get(token) {
            Some(expires_at) if *expires_at > Instant::now() => true,
            Some(_) => {
                sessions.remove(token);
                false
            }
            None => false,
        }
    }

    fn close_session(&self, token: &str) {
        self.sessions.lock().unwrap().remove(token);
    }
}

/// Global (not per-IP) lockout: after `LOCKOUT_THRESHOLD` consecutive failures
/// every login is rejected for the lockout window. Client IPs are unreliable
/// behind proxies, and a global lock is strictly safer for a single admin.
struct LoginLimiter {
    consecutive_failures: u32,
    locked_until: Option<Instant>,
    lockout: Duration,
}

impl LoginLimiter {
    fn new(lockout: Duration) -> Self {
        Self {
            consecutive_failures: 0,
            locked_until: None,
            lockout,
        }
    }

    fn is_locked(&mut self) -> bool {
        match self.locked_until {
            Some(until) if until > Instant::now() => true,
            Some(_) => {
                self.locked_until = None;
                false
            }
            None => false,
        }
    }

    fn register_failure(&mut self) {
        self.consecutive_failures += 1;
        if self.consecutive_failures >= LOCKOUT_THRESHOLD {
            self.locked_until = Some(Instant::now() + self.lockout);
            self.consecutive_failures = 0;
        }
    }

    fn reset(&mut self) {
        self.consecutive_failures = 0;
        self.locked_until = None;
    }
}

fn random_token() -> String {
    use argon2::password_hash::rand_core::RngCore;

    let mut bytes = [0u8; 32];
    argon2::password_hash::rand_core::OsRng.fill_bytes(&mut bytes);
    bytes.iter().fold(String::with_capacity(64), |mut out, b| {
        use std::fmt::Write;
        let _ = write!(out, "{b:02x}");
        out
    })
}

/// The admin console router, mounted at `/admin`. When no admin password is
/// configured the prefix is still reserved but everything responds 404.
pub(crate) fn admin_router(shared: &SharedGatewayState) -> Router {
    if shared.load().admin_password.is_none() {
        return disabled_router();
    }
    admin_router_with_state(AdminState::new(shared.clone()))
}

fn disabled_router() -> Router {
    Router::new().fallback(|| async {
        (
            StatusCode::NOT_FOUND,
            "admin console is disabled; set admin_password in the gateway config to enable it",
        )
    })
}

fn admin_router_with_state(state: AdminState) -> Router {
    Router::new()
        .route(
            "/api/config",
            get(config_api::get_config).put(config_api::put_config),
        )
        .route("/api/usage", get(usage_api::list_usage))
        .route("/api/usage/summary", get(usage_api::usage_summary))
        .route("/api/logout", post(logout))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_session,
        ))
        .route("/api/login", post(login))
        .fallback(assets::spa_fallback)
        .with_state(state)
}

async fn require_session(
    State(state): State<AdminState>,
    request: Request,
    next: Next,
) -> Response {
    let authorized = session_cookie(request.headers())
        .is_some_and(|token| state.session_is_valid(&token));
    if !authorized {
        return (StatusCode::UNAUTHORIZED, "admin session required").into_response();
    }
    next.run(request).await
}

fn session_cookie(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .filter_map(|pair| pair.trim().strip_prefix(SESSION_COOKIE))
        .find_map(|rest| rest.strip_prefix('='))
        .map(str::to_owned)
}

fn set_cookie_header(token: &str, max_age_secs: i64) -> String {
    format!(
        "{SESSION_COOKIE}={token}; HttpOnly; SameSite=Strict; Path=/admin; Max-Age={max_age_secs}"
    )
}

#[derive(Deserialize)]
struct LoginRequest {
    password: String,
}

async fn login(State(state): State<AdminState>, Json(request): Json<LoginRequest>) -> Response {
    if state.limiter.lock().unwrap().is_locked() {
        warn!("admin login rejected: lockout window active");
        return (
            StatusCode::TOO_MANY_REQUESTS,
            "too many failed login attempts; try again later",
        )
            .into_response();
    }

    let Some(stored) = state.shared.load().admin_password.clone() else {
        warn!("admin login attempted but no admin_password is configured");
        return (StatusCode::UNAUTHORIZED, "invalid password").into_response();
    };

    if !password::verify_password(&stored, &request.password) {
        state.limiter.lock().unwrap().register_failure();
        warn!("admin login failed: invalid password");
        return (StatusCode::UNAUTHORIZED, "invalid password").into_response();
    }

    state.limiter.lock().unwrap().reset();
    let token = state.open_session();
    let ttl_secs = state.session_ttl.as_secs() as i64;
    (
        [(header::SET_COOKIE, set_cookie_header(&token, ttl_secs))],
        Json(serde_json::json!({ "ok": true })),
    )
        .into_response()
}

async fn logout(State(state): State<AdminState>, headers: HeaderMap) -> Response {
    if let Some(token) = session_cookie(&headers) {
        state.close_session(&token);
    }
    (
        [(header::SET_COOKIE, set_cookie_header("", 0))],
        Json(serde_json::json!({ "ok": true })),
    )
        .into_response()
}
