use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use super::{AdminState, SessionIdentity, password, require_admin};
use crate::account::{
    ApiKeyRecord, Conflict, KeyStatus, Role, UserStatus, generate_api_key, hash_api_key_hex,
    key_prefix,
};

const MIN_PASSWORD_LEN: usize = 8;

pub(crate) async fn me(Extension(identity): Extension<SessionIdentity>) -> Response {
    Json(json!({
        "user_id": identity.user_id,
        "username": identity.username,
        "role": identity.role,
    }))
    .into_response()
}

#[derive(Deserialize)]
pub(crate) struct ChangePasswordRequest {
    current_password: String,
    new_password: String,
}

pub(crate) async fn change_own_password(
    State(state): State<AdminState>,
    Extension(identity): Extension<SessionIdentity>,
    Json(request): Json<ChangePasswordRequest>,
) -> Response {
    if let Err(response) = validate_password(&request.new_password) {
        return response;
    }
    let user = match state.store().get_user(identity.user_id).await {
        Ok(Some(user)) => user,
        Ok(None) => return unauthorized("session user no longer exists"),
        Err(error) => return internal(error, "failed to load user"),
    };
    if !password::verify_password(&user.password_hash, &request.current_password) {
        return unauthorized("current password is incorrect");
    }
    let hash = match password::hash_password(&request.new_password) {
        Ok(hash) => hash,
        Err(error) => return internal(error, "failed to hash password"),
    };
    match state.store().set_password_hash(identity.user_id, &hash).await {
        Ok(true) => Json(json!({ "ok": true })).into_response(),
        Ok(false) => unauthorized("session user no longer exists"),
        Err(error) => internal(error, "failed to update password"),
    }
}

pub(crate) async fn list_users(
    State(state): State<AdminState>,
    Extension(identity): Extension<SessionIdentity>,
) -> Response {
    if let Err(response) = require_admin(&identity) {
        return response;
    }
    match state.store().list_users().await {
        Ok(users) => Json(json!({ "users": users })).into_response(),
        Err(error) => internal(error, "failed to list users"),
    }
}

#[derive(Deserialize)]
pub(crate) struct CreateUserRequest {
    username: String,
    password: String,
    role: String,
}

pub(crate) async fn create_user(
    State(state): State<AdminState>,
    Extension(identity): Extension<SessionIdentity>,
    Json(request): Json<CreateUserRequest>,
) -> Response {
    if let Err(response) = require_admin(&identity) {
        return response;
    }
    let username = request.username.trim();
    if username.is_empty() || username.len() > 64 {
        return bad_request("username must be 1-64 characters");
    }
    if let Err(response) = validate_password(&request.password) {
        return response;
    }
    let Some(role) = Role::parse(&request.role) else {
        return bad_request("role must be 'admin' or 'user'");
    };
    let hash = match password::hash_password(&request.password) {
        Ok(hash) => hash,
        Err(error) => return internal(error, "failed to hash password"),
    };
    match state.store().create_user(username, &hash, role).await {
        Ok(user) => (StatusCode::CREATED, Json(json!({ "user": user }))).into_response(),
        Err(error) if error.is::<Conflict>() => {
            (StatusCode::CONFLICT, Json(json!({ "error": error.to_string() }))).into_response()
        }
        Err(error) => internal(error, "failed to create user"),
    }
}

pub(crate) async fn disable_user(
    State(state): State<AdminState>,
    Extension(identity): Extension<SessionIdentity>,
    Path(id): Path<Uuid>,
) -> Response {
    if let Err(response) = require_admin(&identity) {
        return response;
    }
    if id == identity.user_id {
        return bad_request("cannot disable your own account");
    }
    match state.store().set_user_status(id, UserStatus::Disabled).await {
        Ok(true) => {
            // Sessions die immediately; the user's keys leave the hot-path
            // map on the snapshot refresh below.
            state.drop_user_sessions(id).await;
            refresh_and_respond(&state, json!({ "ok": true })).await
        }
        Ok(false) => not_found("user not found"),
        Err(error) => internal(error, "failed to disable user"),
    }
}

pub(crate) async fn enable_user(
    State(state): State<AdminState>,
    Extension(identity): Extension<SessionIdentity>,
    Path(id): Path<Uuid>,
) -> Response {
    if let Err(response) = require_admin(&identity) {
        return response;
    }
    match state.store().set_user_status(id, UserStatus::Active).await {
        Ok(true) => refresh_and_respond(&state, json!({ "ok": true })).await,
        Ok(false) => not_found("user not found"),
        Err(error) => internal(error, "failed to enable user"),
    }
}

#[derive(Deserialize)]
pub(crate) struct ResetPasswordRequest {
    password: String,
}

pub(crate) async fn reset_password(
    State(state): State<AdminState>,
    Extension(identity): Extension<SessionIdentity>,
    Path(id): Path<Uuid>,
    Json(request): Json<ResetPasswordRequest>,
) -> Response {
    if let Err(response) = require_admin(&identity) {
        return response;
    }
    if let Err(response) = validate_password(&request.password) {
        return response;
    }
    let hash = match password::hash_password(&request.password) {
        Ok(hash) => hash,
        Err(error) => return internal(error, "failed to hash password"),
    };
    match state.store().set_password_hash(id, &hash).await {
        Ok(true) => {
            // A reset password implies the old credential is compromised or
            // forgotten either way existing sessions should not survive.
            state.drop_user_sessions(id).await;
            Json(json!({ "ok": true })).into_response()
        }
        Ok(false) => not_found("user not found"),
        Err(error) => internal(error, "failed to reset password"),
    }
}

pub(crate) async fn list_keys(
    State(state): State<AdminState>,
    Extension(identity): Extension<SessionIdentity>,
) -> Response {
    let owner = match identity.role {
        Role::Admin => None,
        Role::User => Some(identity.user_id),
    };
    let keys = match state.store().list_keys(owner).await {
        Ok(keys) => keys,
        Err(error) => return internal(error, "failed to list keys"),
    };

    // Admins see every key; attach owner usernames so the console can label
    // them without a second round trip.
    let usernames: std::collections::HashMap<Uuid, String> = if identity.role == Role::Admin {
        match state.store().list_users().await {
            Ok(users) => users.into_iter().map(|user| (user.id, user.username)).collect(),
            Err(error) => return internal(error, "failed to list users"),
        }
    } else {
        std::collections::HashMap::new()
    };

    let snapshot = state.shared().load();
    let mut rows = Vec::with_capacity(keys.len());
    for key in &keys {
        let spent = snapshot
            .spend_counter
            .get(crate::quota::Scope::Key(key.id))
            .await;
        rows.push(key_json(
            key,
            usernames.get(&key.user_id).map(String::as_str),
            Some(spent),
        ));
    }
    Json(json!({ "keys": rows })).into_response()
}

#[derive(Deserialize)]
pub(crate) struct CreateKeyRequest {
    name: String,
    /// Model names the key may call; absent or `null` = every enabled model.
    #[serde(default)]
    allowed_models: Option<Vec<String>>,
    /// Cumulative USD spend cap; absent or `null` = uncapped.
    #[serde(default)]
    spend_cap_usd: Option<String>,
}

pub(crate) async fn create_key(
    State(state): State<AdminState>,
    Extension(identity): Extension<SessionIdentity>,
    Json(request): Json<CreateKeyRequest>,
) -> Response {
    let name = request.name.trim();
    if name.is_empty() || name.len() > 100 {
        return bad_request("key name must be 1-100 characters");
    }
    let allowed_models = match normalize_allowed_models(request.allowed_models) {
        Ok(allowed_models) => allowed_models,
        Err(response) => return response,
    };
    let spend_cap = match parse_spend_cap(request.spend_cap_usd.as_deref()) {
        Ok(spend_cap) => spend_cap,
        Err(response) => return response,
    };

    let plaintext = generate_api_key();
    let created = state
        .store()
        .create_key(
            identity.user_id,
            name,
            &hash_api_key_hex(&plaintext),
            &key_prefix(&plaintext),
            allowed_models.as_deref(),
            spend_cap,
        )
        .await;
    match created {
        Ok(record) => {
            refresh_and_respond(
                &state,
                json!({
                    // The only response that ever carries the plaintext key.
                    "key": plaintext,
                    "record": key_json(&record, None, None),
                }),
            )
            .await
        }
        Err(error) => internal(error, "failed to create key"),
    }
}

#[derive(Deserialize)]
pub(crate) struct UpdateKeyRequest {
    /// The replacement allowed-models list; `null` = unrestricted.
    #[serde(default)]
    allowed_models: Option<Vec<String>>,
    /// The replacement spend cap; `null` = uncapped.
    #[serde(default)]
    spend_cap_usd: Option<String>,
}

/// Replaces a key's limits — allowed models and spend cap (owner or admin).
pub(crate) async fn update_key(
    State(state): State<AdminState>,
    Extension(identity): Extension<SessionIdentity>,
    Path(id): Path<Uuid>,
    Json(request): Json<UpdateKeyRequest>,
) -> Response {
    let key = match state.store().get_key(id).await {
        Ok(Some(key)) => key,
        Ok(None) => return not_found("key not found"),
        Err(error) => return internal(error, "failed to load key"),
    };
    if identity.role != Role::Admin && key.user_id != identity.user_id {
        // 404 rather than 403: a non-owner cannot probe other users' key ids.
        return not_found("key not found");
    }
    let allowed_models = match normalize_allowed_models(request.allowed_models) {
        Ok(allowed_models) => allowed_models,
        Err(response) => return response,
    };
    let spend_cap = match parse_spend_cap(request.spend_cap_usd.as_deref()) {
        Ok(spend_cap) => spend_cap,
        Err(response) => return response,
    };
    match state
        .store()
        .set_key_limits(id, allowed_models.as_deref(), spend_cap)
        .await
    {
        Ok(true) => refresh_and_respond(&state, json!({ "ok": true })).await,
        Ok(false) => not_found("key not found"),
        Err(error) => internal(error, "failed to update key"),
    }
}

/// Parses an optional decimal spend cap; empty string = uncapped.
fn parse_spend_cap(value: Option<&str>) -> Result<Option<rust_decimal::Decimal>, Response> {
    let Some(text) = value.map(str::trim).filter(|text| !text.is_empty()) else {
        return Ok(None);
    };
    let cap: rust_decimal::Decimal = text
        .parse()
        .map_err(|_| bad_request("spend_cap_usd must be a decimal number"))?;
    if cap <= rust_decimal::Decimal::ZERO {
        return Err(bad_request("spend_cap_usd must be positive"));
    }
    Ok(Some(cap))
}

/// Trims, deduplicates, and bounds an allowed-models list. `None` passes
/// through (unrestricted); an empty list is rejected — the console sends
/// `null` for "all models", so an empty list is always a client mistake that
/// would otherwise brick the key.
fn normalize_allowed_models(
    allowed_models: Option<Vec<String>>,
) -> Result<Option<Vec<String>>, Response> {
    let Some(names) = allowed_models else {
        return Ok(None);
    };
    let mut normalized = Vec::new();
    for name in names {
        let name = name.trim();
        if name.is_empty() {
            return Err(bad_request("allowed_models cannot contain empty names"));
        }
        if !normalized.iter().any(|existing| existing == name) {
            normalized.push(name.to_owned());
        }
    }
    if normalized.is_empty() {
        return Err(bad_request(
            "allowed_models cannot be an empty list; send null to allow every model",
        ));
    }
    if normalized.len() > 200 {
        return Err(bad_request("allowed_models cannot exceed 200 entries"));
    }
    Ok(Some(normalized))
}

pub(crate) async fn revoke_key(
    State(state): State<AdminState>,
    Extension(identity): Extension<SessionIdentity>,
    Path(id): Path<Uuid>,
) -> Response {
    let key = match state.store().get_key(id).await {
        Ok(Some(key)) => key,
        Ok(None) => return not_found("key not found"),
        Err(error) => return internal(error, "failed to load key"),
    };
    if identity.role != Role::Admin && key.user_id != identity.user_id {
        // 404 rather than 403: a non-owner cannot probe other users' key ids.
        return not_found("key not found");
    }
    if key.status == KeyStatus::Revoked {
        return Json(json!({ "ok": true })).into_response();
    }
    match state.store().revoke_key(id).await {
        Ok(true) => refresh_and_respond(&state, json!({ "ok": true })).await,
        Ok(false) => not_found("key not found"),
        Err(error) => internal(error, "failed to revoke key"),
    }
}

/// Refreshes the hot-path key map after a mutation, reporting a refresh
/// failure in the response (the database write has already committed).
async fn refresh_and_respond(state: &AdminState, body: serde_json::Value) -> Response {
    match state.refresh_api_keys().await {
        Ok(()) => Json(body).into_response(),
        Err(error) => {
            tracing::error!(
                error = format!("{error:#}"),
                "account mutation committed but key snapshot refresh failed"
            );
            let mut body = body;
            if let Some(object) = body.as_object_mut() {
                object.insert(
                    "warning".to_owned(),
                    json!("saved, but applying the change to the gateway failed; it will apply on next restart"),
                );
            }
            Json(body).into_response()
        }
    }
}

fn key_json(
    record: &ApiKeyRecord,
    username: Option<&str>,
    spent: Option<rust_decimal::Decimal>,
) -> serde_json::Value {
    let mut value = json!({
        "id": record.id,
        "user_id": record.user_id,
        "prefix": record.prefix,
        "name": record.name,
        "status": record.status,
        "created_at": record.created_at,
        "last_used_at": record.last_used_at,
        "allowed_models": record.allowed_models,
        "spend_cap_usd": record.spend_cap_usd.map(|cap| cap.to_string()),
        "spent_usd": spent.map(|spent| spent.to_string()),
    });
    if let (Some(username), Some(object)) = (username, value.as_object_mut()) {
        object.insert("username".to_owned(), json!(username));
    }
    value
}

fn validate_password(password: &str) -> Result<(), Response> {
    if password.len() < MIN_PASSWORD_LEN {
        return Err(bad_request("password must be at least 8 characters"));
    }
    Ok(())
}

fn bad_request(message: &str) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": message }))).into_response()
}

fn unauthorized(message: &str) -> Response {
    (StatusCode::UNAUTHORIZED, Json(json!({ "error": message }))).into_response()
}

fn not_found(message: &str) -> Response {
    (StatusCode::NOT_FOUND, Json(json!({ "error": message }))).into_response()
}

fn internal(error: anyhow::Error, message: &str) -> Response {
    tracing::warn!(error = format!("{error:#}"), message);
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": message })),
    )
        .into_response()
}
