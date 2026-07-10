mod account_api;
mod assets;
mod catalog_api;
mod credits_api;
mod models_api;
pub(crate) mod password;
mod session;
mod usage_api;

#[cfg(test)]
mod tests;

use std::{sync::Arc, time::Duration};

use axum::{
    Json, Router,
    extract::{Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use serde::{Deserialize, Serialize};
use tracing::warn;
use uuid::Uuid;

use crate::{
    account::{AccountStore, Role, UserStatus},
    catalog::CatalogStore,
    pubsub::ConfigNotifier,
    reload::SharedGatewayState,
};

pub use password::bootstrap_admin_password;
pub(crate) use session::{MemorySessionStore, RedisSessionStore, SessionStore};
#[cfg(test)]
use session::LOCKOUT_WINDOW;

const SESSION_COOKIE: &str = "lite_agentify_admin";
const SESSION_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// The authenticated caller attached to each admin request by the session
/// middleware. Serialized as the session payload in Redis mode.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct SessionIdentity {
    pub user_id: Uuid,
    pub username: String,
    pub role: Role,
}

/// State for the admin console. Sessions and the login limiter live behind
/// [`SessionStore`] — beside, not inside, the arc-swapped gateway state — so
/// a config hot reload never invalidates sessions.
#[derive(Clone)]
pub(crate) struct AdminState {
    shared: SharedGatewayState,
    store: Arc<dyn AccountStore>,
    catalog: Arc<dyn CatalogStore>,
    quota: Arc<dyn crate::quota::QuotaStore>,
    sessions: Arc<dyn SessionStore>,
    notifier: Option<ConfigNotifier>,
    session_ttl: Duration,
}

impl AdminState {
    fn new(
        shared: SharedGatewayState,
        store: Arc<dyn AccountStore>,
        catalog: Arc<dyn CatalogStore>,
        quota: Arc<dyn crate::quota::QuotaStore>,
        sessions: Arc<dyn SessionStore>,
        notifier: Option<ConfigNotifier>,
    ) -> Self {
        Self {
            shared,
            store,
            catalog,
            quota,
            sessions,
            notifier,
            session_ttl: SESSION_TTL,
        }
    }

    #[cfg(test)]
    fn with_timing(
        shared: SharedGatewayState,
        store: Arc<dyn AccountStore>,
        catalog: Arc<dyn CatalogStore>,
        quota: Arc<dyn crate::quota::QuotaStore>,
        session_ttl: Duration,
        lockout: Duration,
    ) -> Self {
        Self {
            shared,
            store,
            catalog,
            quota,
            sessions: Arc::new(MemorySessionStore::new(lockout)),
            notifier: None,
            session_ttl,
        }
    }

    pub(crate) fn shared(&self) -> &SharedGatewayState {
        &self.shared
    }

    pub(crate) fn store(&self) -> &Arc<dyn AccountStore> {
        &self.store
    }

    pub(crate) fn catalog(&self) -> &Arc<dyn CatalogStore> {
        &self.catalog
    }

    pub(crate) fn quota(&self) -> &Arc<dyn crate::quota::QuotaStore> {
        &self.quota
    }

    /// Announces a snapshot-affecting mutation on the reserved pub/sub
    /// channel (no-op without Redis; see `pubsub.rs`).
    fn notify_config_changed(&self, what: &'static str) {
        if let Some(notifier) = &self.notifier {
            notifier.publish(what);
        }
    }

    /// Reloads the per-user granted sums into the snapshot after a grant
    /// mutation, so balance checks see the new credit immediately.
    pub(crate) async fn refresh_granted(&self) -> anyhow::Result<()> {
        let granted = self.quota.grant_sums().await?;
        self.shared.store_granted(granted);
        self.notify_config_changed("granted");
        Ok(())
    }

    /// Rebuilds the gateway snapshot from the current database catalog after a
    /// provider or pricing mutation. Returns an error (without swapping) when
    /// the new catalog fails validation, e.g. a route references a deleted
    /// provider.
    pub(crate) async fn refresh_catalog(&self) -> anyhow::Result<()> {
        let snapshot = self.catalog.snapshot().await?;
        self.shared.store_catalog(snapshot)?;
        self.notify_config_changed("catalog");
        Ok(())
    }

    /// Reloads the hot-path key map from the store after an account mutation.
    /// A failure is logged and surfaced so callers can report it: the database
    /// write has already committed and the next rebuild converges.
    pub(crate) async fn refresh_api_keys(&self) -> anyhow::Result<()> {
        let map = self.store.active_key_map().await?;
        self.shared.store_api_keys(map);
        self.notify_config_changed("api_keys");
        Ok(())
    }

    /// Invalidates every session belonging to `user_id` (user disabled or
    /// password reset). A backend failure is logged loudly: the sessions can
    /// only resurface after the backend recovers, since reads during the
    /// outage fail closed.
    pub(crate) async fn drop_user_sessions(&self, user_id: Uuid) {
        if let Err(error) = self.sessions.remove_user(user_id).await {
            tracing::error!(
                error = format!("{error:#}"),
                %user_id,
                "failed to invalidate the user's sessions; they lapse at their TTL"
            );
        }
    }

    async fn open_session(&self, identity: SessionIdentity) -> anyhow::Result<String> {
        let token = random_token();
        self.sessions
            .open(&token, &identity, self.session_ttl)
            .await?;
        Ok(token)
    }

    /// Validates a session token; a session-backend outage reads as signed
    /// out (fail closed).
    async fn session_identity(&self, token: &str) -> Option<SessionIdentity> {
        match self.sessions.get(token).await {
            Ok(identity) => identity,
            Err(error) => {
                warn!(
                    error = format!("{error:#}"),
                    "session lookup failed; failing closed"
                );
                None
            }
        }
    }

    async fn close_session(&self, token: &str) {
        self.sessions.remove(token).await;
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

/// The admin console router, mounted at `/admin`. Always enabled: user
/// accounts live in the database and the bootstrap admin is guaranteed to
/// exist after startup.
pub(crate) fn admin_router(
    shared: &SharedGatewayState,
    store: Arc<dyn AccountStore>,
    catalog: Arc<dyn CatalogStore>,
    quota: Arc<dyn crate::quota::QuotaStore>,
    sessions: Arc<dyn SessionStore>,
    notifier: Option<ConfigNotifier>,
) -> Router {
    admin_router_with_state(AdminState::new(
        shared.clone(),
        store,
        catalog,
        quota,
        sessions,
        notifier,
    ))
}

fn admin_router_with_state(state: AdminState) -> Router {
    Router::new()
        .route("/api/usage", get(usage_api::list_usage))
        .route("/api/usage/summary", get(usage_api::usage_summary))
        .route("/api/me", get(account_api::me))
        .route("/api/me/password", post(account_api::change_own_password))
        .route(
            "/api/users",
            get(account_api::list_users).post(account_api::create_user),
        )
        .route("/api/users/{id}/disable", post(account_api::disable_user))
        .route("/api/users/{id}/enable", post(account_api::enable_user))
        .route(
            "/api/users/{id}/reset-password",
            post(account_api::reset_password),
        )
        .route(
            "/api/keys",
            get(account_api::list_keys).post(account_api::create_key),
        )
        .route("/api/keys/{id}", put(account_api::update_key))
        .route("/api/keys/{id}/revoke", post(account_api::revoke_key))
        .route(
            "/api/providers",
            get(catalog_api::list_providers).post(catalog_api::create_provider),
        )
        .route(
            "/api/providers/{id}",
            put(catalog_api::update_provider).delete(catalog_api::delete_provider),
        )
        .route(
            "/api/providers/{id}/reveal",
            post(catalog_api::reveal_provider_key),
        )
        .route(
            "/api/pricing",
            get(catalog_api::list_pricing).post(catalog_api::create_pricing),
        )
        .route(
            "/api/pricing/{id}",
            put(catalog_api::update_pricing).delete(catalog_api::delete_pricing),
        )
        .route(
            "/api/models",
            get(models_api::list_models).post(models_api::create_model),
        )
        .route("/api/models/names", get(models_api::list_model_names))
        .route(
            "/api/models/{name}",
            put(models_api::update_model).delete(models_api::delete_model),
        )
        .route(
            "/api/credits",
            get(credits_api::list_balances),
        )
        .route("/api/credits/grants", post(credits_api::create_grant))
        .route("/api/credits/ledger", get(credits_api::list_ledger))
        .route("/api/me/balance", get(credits_api::my_balance))
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
    mut request: Request,
    next: Next,
) -> Response {
    let identity = match session_cookie(request.headers()) {
        Some(token) => state.session_identity(&token).await,
        None => None,
    };
    let Some(identity) = identity else {
        return (StatusCode::UNAUTHORIZED, "admin session required").into_response();
    };
    request.extensions_mut().insert(identity);
    next.run(request).await
}

/// Guard for admin-only endpoints: `Err` carries the 403 response.
pub(crate) fn require_admin(identity: &SessionIdentity) -> Result<(), Response> {
    if identity.role == Role::Admin {
        Ok(())
    } else {
        Err((StatusCode::FORBIDDEN, "admin role required").into_response())
    }
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
    username: String,
    password: String,
}

/// Identical rejection for unknown username, disabled user, and wrong
/// password, with a dummy argon2 verification on the missing-user path so
/// response timing does not reveal whether a username exists.
async fn login(State(state): State<AdminState>, Json(request): Json<LoginRequest>) -> Response {
    if state.sessions.is_locked(&request.username).await {
        warn!(username = %request.username, "login rejected: lockout window active");
        return (
            StatusCode::TOO_MANY_REQUESTS,
            "too many failed login attempts; try again later",
        )
            .into_response();
    }

    let user = match state.store.find_user_by_username(&request.username).await {
        Ok(user) => user,
        Err(error) => {
            warn!(error = format!("{error:#}"), "login user lookup failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "login failed").into_response();
        }
    };

    let verified = match &user {
        Some(user) => {
            password::verify_password(&user.password_hash, &request.password)
                && user.status == UserStatus::Active
        }
        None => {
            password::verify_password(dummy_hash(), &request.password);
            false
        }
    };

    if !verified {
        state.sessions.register_failure(&request.username).await;
        warn!(username = %request.username, "login failed: invalid credentials");
        return (StatusCode::UNAUTHORIZED, "invalid credentials").into_response();
    }

    let user = user.expect("verified implies user exists");
    state.sessions.clear_failures(&request.username).await;
    let identity = SessionIdentity {
        user_id: user.id,
        username: user.username.clone(),
        role: user.role,
    };
    let token = match state.open_session(identity).await {
        Ok(token) => token,
        Err(error) => {
            // Fail closed: no session persisted means no cookie is issued.
            warn!(
                error = format!("{error:#}"),
                "login verified but the session could not be stored"
            );
            return (StatusCode::INTERNAL_SERVER_ERROR, "login failed").into_response();
        }
    };
    let ttl_secs = state.session_ttl.as_secs() as i64;
    (
        [(header::SET_COOKIE, set_cookie_header(&token, ttl_secs))],
        Json(serde_json::json!({
            "ok": true,
            "username": user.username,
            "role": user.role,
        })),
    )
        .into_response()
}

/// A constant argon2id hash of an unguessable value, verified against on the
/// unknown-username path so both login failures cost the same time.
fn dummy_hash() -> &'static str {
    use std::sync::OnceLock;
    static DUMMY: OnceLock<String> = OnceLock::new();
    DUMMY.get_or_init(|| {
        password::hash_password(&random_token()).unwrap_or_else(|_| "$argon2id$invalid".to_owned())
    })
}

async fn logout(State(state): State<AdminState>, headers: HeaderMap) -> Response {
    if let Some(token) = session_cookie(&headers) {
        state.close_session(&token).await;
    }
    (
        [(header::SET_COOKIE, set_cookie_header("", 0))],
        Json(serde_json::json!({ "ok": true })),
    )
        .into_response()
}
