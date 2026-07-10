use axum::{
    Extension, Json,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use super::{AdminState, SessionIdentity, require_admin};
use crate::quota::Scope;

/// The three numbers behind every balance display: Σ grants, cumulative spend
/// (from the live counter), and their difference.
async fn balance_parts(state: &AdminState, user_id: Uuid) -> (Decimal, Decimal, Decimal) {
    let snapshot = state.shared().load();
    let granted = snapshot.granted.get(&user_id).copied().unwrap_or_default();
    let spent = snapshot.spend_counter.get(Scope::User(user_id)).await;
    (granted, spent, granted - spent)
}

/// Own balance, visible to every signed-in user (dashboard card).
pub(crate) async fn my_balance(
    State(state): State<AdminState>,
    Extension(identity): Extension<SessionIdentity>,
) -> Response {
    let (granted, spent, balance) = balance_parts(&state, identity.user_id).await;
    Json(json!({
        "granted": granted.to_string(),
        "spent": spent.to_string(),
        "balance": balance.to_string(),
    }))
    .into_response()
}

/// Admin: every user with granted/spent/balance.
pub(crate) async fn list_balances(
    State(state): State<AdminState>,
    Extension(identity): Extension<SessionIdentity>,
) -> Response {
    if let Err(response) = require_admin(&identity) {
        return response;
    }
    let users = match state.store().list_users().await {
        Ok(users) => users,
        Err(error) => return internal(error, "failed to list users"),
    };
    let mut rows = Vec::with_capacity(users.len());
    for user in users {
        let (granted, spent, balance) = balance_parts(&state, user.id).await;
        rows.push(json!({
            "user_id": user.id,
            "username": user.username,
            "status": user.status,
            "granted": granted.to_string(),
            "spent": spent.to_string(),
            "balance": balance.to_string(),
        }));
    }
    Json(json!({ "balances": rows })).into_response()
}

#[derive(Deserialize)]
pub(crate) struct GrantBody {
    user_id: Uuid,
    /// Positive to grant, negative to correct. Decimal as a string.
    amount_usd: String,
    #[serde(default)]
    note: Option<String>,
}

pub(crate) async fn create_grant(
    State(state): State<AdminState>,
    Extension(identity): Extension<SessionIdentity>,
    Json(body): Json<GrantBody>,
) -> Response {
    if let Err(response) = require_admin(&identity) {
        return response;
    }
    let Ok(amount) = body.amount_usd.trim().parse::<Decimal>() else {
        return bad_request("amount_usd must be a decimal number");
    };
    if amount == Decimal::ZERO {
        return bad_request("amount_usd cannot be zero");
    }
    let note = body.note.as_deref().map(str::trim).filter(|note| !note.is_empty());
    // Grants must target an existing user (the FK would also catch it, but a
    // clean 404 beats a 500).
    match state.store().get_user(body.user_id).await {
        Ok(Some(_)) => {}
        Ok(None) => return not_found("user not found"),
        Err(error) => return internal(error, "failed to load user"),
    }

    let grant = match state
        .quota()
        .append_grant(body.user_id, amount, note, identity.user_id)
        .await
    {
        Ok(grant) => grant,
        Err(error) => return internal(error, "failed to append grant"),
    };

    // Refresh the snapshot's granted map so the new credit is enforceable
    // immediately. The ledger row has committed either way.
    let mut payload = json!({ "grant": grant });
    if let Err(error) = state.refresh_granted().await {
        tracing::error!(
            error = format!("{error:#}"),
            "grant committed but snapshot refresh failed"
        );
        if let Some(object) = payload.as_object_mut() {
            object.insert(
                "warning".to_owned(),
                json!("saved, but applying the balance to the gateway failed; it will apply on next restart"),
            );
        }
    }
    (StatusCode::CREATED, Json(payload)).into_response()
}

#[derive(Deserialize)]
pub(crate) struct LedgerQuery {
    #[serde(default)]
    user_id: Option<Uuid>,
    #[serde(default)]
    limit: Option<u64>,
}

pub(crate) async fn list_ledger(
    State(state): State<AdminState>,
    Extension(identity): Extension<SessionIdentity>,
    Query(query): Query<LedgerQuery>,
) -> Response {
    if let Err(response) = require_admin(&identity) {
        return response;
    }
    let limit = query.limit.unwrap_or(100).min(1000);
    match state.quota().list_grants(query.user_id, limit).await {
        Ok(grants) => {
            // Attach usernames so the console can label rows directly.
            let usernames: std::collections::HashMap<Uuid, String> =
                match state.store().list_users().await {
                    Ok(users) => users
                        .into_iter()
                        .map(|user| (user.id, user.username))
                        .collect(),
                    Err(error) => return internal(error, "failed to list users"),
                };
            let rows: Vec<serde_json::Value> = grants
                .iter()
                .map(|grant| {
                    json!({
                        "id": grant.id,
                        "user_id": grant.user_id,
                        "username": usernames.get(&grant.user_id),
                        "amount_usd": grant.amount_usd.to_string(),
                        "note": grant.note,
                        "granted_by": usernames.get(&grant.granted_by),
                        "created_at": grant.created_at,
                    })
                })
                .collect();
            Json(json!({ "grants": rows })).into_response()
        }
        Err(error) => internal(error, "failed to list grants"),
    }
}

fn bad_request(message: &str) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": message }))).into_response()
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
