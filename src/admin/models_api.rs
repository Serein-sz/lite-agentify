use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::json;

use super::{AdminState, SessionIdentity, require_admin};
use crate::catalog::{CatalogConflict, ModelConfig, uncovered_deployments};

fn model_json(model: &ModelConfig, uncovered: &[String]) -> serde_json::Value {
    json!({
        "name": model.name,
        "enabled": model.enabled,
        "created_at": model.created_at,
        "deployments": model
            .deployments
            .iter()
            .map(|deployment| json!({
                "id": deployment.id,
                "provider_id": deployment.provider_id,
                "upstream_model": deployment.upstream_model,
            }))
            .collect::<Vec<_>>(),
        // Deployments without a pricing rule, so the console can explain why
        // enabling would be rejected.
        "uncovered": uncovered,
    })
}

async fn model_rows(state: &AdminState) -> anyhow::Result<Vec<serde_json::Value>> {
    let models = state.catalog().list_models().await?;
    let pricing: Vec<_> = state
        .catalog()
        .list_pricing()
        .await?
        .into_iter()
        .map(|record| record.config)
        .collect();
    Ok(models
        .iter()
        .map(|model| {
            let uncovered: Vec<String> = uncovered_deployments(model, &pricing)
                .iter()
                .map(|deployment| {
                    format!("{}:{}", deployment.provider_id, deployment.upstream_model)
                })
                .collect();
            model_json(model, &uncovered)
        })
        .collect())
}

pub(crate) async fn list_models(
    State(state): State<AdminState>,
    Extension(identity): Extension<SessionIdentity>,
) -> Response {
    if let Err(response) = require_admin(&identity) {
        return response;
    }
    match model_rows(&state).await {
        Ok(rows) => Json(json!({ "models": rows })).into_response(),
        Err(error) => internal(error, "failed to list models"),
    }
}

/// Enabled model names — readable by every signed-in user, so the key editor
/// can offer the allowed-models picker without exposing admin-only detail.
pub(crate) async fn list_model_names(State(state): State<AdminState>) -> Response {
    match state.catalog().list_models().await {
        Ok(models) => {
            let names: Vec<&str> = models
                .iter()
                .filter(|model| model.enabled)
                .map(|model| model.name.as_str())
                .collect();
            Json(json!({ "models": names })).into_response()
        }
        Err(error) => internal(error, "failed to list models"),
    }
}

#[derive(Deserialize)]
pub(crate) struct DeploymentBody {
    provider_id: String,
    upstream_model: String,
}

#[derive(Deserialize)]
pub(crate) struct CreateModelBody {
    name: String,
    #[serde(default)]
    deployments: Vec<DeploymentBody>,
    #[serde(default)]
    enabled: bool,
}

#[derive(Deserialize)]
pub(crate) struct UpdateModelBody {
    deployments: Vec<DeploymentBody>,
    enabled: bool,
}

/// Validates a deployment list against the current provider set. Returns the
/// `(provider_id, upstream_model)` pairs the store expects.
async fn validate_deployments(
    state: &AdminState,
    deployments: Vec<DeploymentBody>,
) -> Result<Vec<(String, String)>, Response> {
    let providers = state
        .catalog()
        .list_providers()
        .await
        .map_err(|error| internal(error, "failed to load providers"))?;
    let mut resolved = Vec::new();
    for deployment in deployments {
        let provider_id = deployment.provider_id.trim().to_owned();
        let upstream_model = deployment.upstream_model.trim().to_owned();
        if provider_id.is_empty() {
            return Err(bad_request("deployment provider cannot be empty"));
        }
        if upstream_model.is_empty() {
            return Err(bad_request("deployment upstream model cannot be empty"));
        }
        if !providers.iter().any(|provider| provider.id == provider_id) {
            return Err(bad_request(&format!(
                "deployment references unknown provider '{provider_id}'"
            )));
        }
        if resolved
            .iter()
            .any(|(existing, _): &(String, String)| *existing == provider_id)
        {
            return Err(bad_request(&format!(
                "provider '{provider_id}' appears more than once in the chain"
            )));
        }
        resolved.push((provider_id, upstream_model));
    }
    Ok(resolved)
}

/// The pricing-coverage listing gate: enabling requires a non-empty, fully
/// priced deployment chain. Returns 409 naming the first uncovered deployment.
async fn ensure_listable(
    state: &AdminState,
    name: &str,
    deployments: &[(String, String)],
) -> Result<(), Response> {
    if deployments.is_empty() {
        return Err(conflict(format!(
            "model '{name}' cannot be enabled without at least one deployment"
        )));
    }
    let pricing: Vec<_> = state
        .catalog()
        .list_pricing()
        .await
        .map_err(|error| internal(error, "failed to load pricing"))?
        .into_iter()
        .map(|record| record.config)
        .collect();
    let candidate = ModelConfig {
        name: name.to_owned(),
        enabled: true,
        created_at: chrono::Utc::now(),
        deployments: deployments
            .iter()
            .map(|(provider_id, upstream_model)| crate::catalog::DeploymentConfig {
                id: uuid::Uuid::new_v4(),
                provider_id: provider_id.clone(),
                upstream_model: upstream_model.clone(),
            })
            .collect(),
    };
    if let Some(deployment) = uncovered_deployments(&candidate, &pricing).first() {
        return Err(conflict(format!(
            "model '{name}' cannot be listed: deployment '{}:{}' has no pricing rule; \
             add pricing first (wildcards count)",
            deployment.provider_id, deployment.upstream_model
        )));
    }
    Ok(())
}

pub(crate) async fn create_model(
    State(state): State<AdminState>,
    Extension(identity): Extension<SessionIdentity>,
    Json(body): Json<CreateModelBody>,
) -> Response {
    if let Err(response) = require_admin(&identity) {
        return response;
    }
    let name = body.name.trim().to_owned();
    if name.is_empty() || name.len() > 200 {
        return bad_request("model name must be 1-200 characters");
    }
    let deployments = match validate_deployments(&state, body.deployments).await {
        Ok(deployments) => deployments,
        Err(response) => return response,
    };
    if body.enabled
        && let Err(response) = ensure_listable(&state, &name, &deployments).await
    {
        return response;
    }

    match state.catalog().create_model(&name).await {
        Ok(_) => {}
        Err(error) if error.is::<CatalogConflict>() => return conflict(error.to_string()),
        Err(error) => return internal(error, "failed to create model"),
    }
    if let Err(error) = state.catalog().set_deployments(&name, &deployments).await {
        return internal(error, "failed to save deployments");
    }
    if body.enabled
        && let Err(error) = state.catalog().set_model_status(&name, true).await
    {
        return internal(error, "failed to enable model");
    }
    refresh_and_respond(&state, StatusCode::CREATED, json!({ "ok": true })).await
}

pub(crate) async fn update_model(
    State(state): State<AdminState>,
    Extension(identity): Extension<SessionIdentity>,
    Path(name): Path<String>,
    Json(body): Json<UpdateModelBody>,
) -> Response {
    if let Err(response) = require_admin(&identity) {
        return response;
    }
    if state.catalog().get_model(&name).await.ok().flatten().is_none() {
        return not_found("model not found");
    }
    let deployments = match validate_deployments(&state, body.deployments).await {
        Ok(deployments) => deployments,
        Err(response) => return response,
    };
    // The gate runs on any mutation that would leave the model enabled: both
    // enabling a draft and editing the chain of an already enabled model.
    if body.enabled
        && let Err(response) = ensure_listable(&state, &name, &deployments).await
    {
        return response;
    }
    if let Err(error) = state.catalog().set_deployments(&name, &deployments).await {
        if error.is::<CatalogConflict>() {
            return conflict(error.to_string());
        }
        return internal(error, "failed to save deployments");
    }
    if let Err(error) = state.catalog().set_model_status(&name, body.enabled).await {
        return internal(error, "failed to update model status");
    }
    refresh_and_respond(&state, StatusCode::OK, json!({ "ok": true })).await
}

pub(crate) async fn delete_model(
    State(state): State<AdminState>,
    Extension(identity): Extension<SessionIdentity>,
    Path(name): Path<String>,
) -> Response {
    if let Err(response) = require_admin(&identity) {
        return response;
    }
    match state.catalog().delete_model(&name).await {
        Ok(true) => refresh_and_respond(&state, StatusCode::OK, json!({ "ok": true })).await,
        Ok(false) => not_found("model not found"),
        Err(error) => internal(error, "failed to delete model"),
    }
}

/// Refreshes the gateway snapshot after a catalog mutation, mirroring the
/// provider/pricing handlers: the database write has committed, so a refresh
/// failure is reported as a warning rather than an error status.
async fn refresh_and_respond(
    state: &AdminState,
    status: StatusCode,
    body: serde_json::Value,
) -> Response {
    match state.refresh_catalog().await {
        Ok(()) => (status, Json(body)).into_response(),
        Err(error) => {
            tracing::error!(
                error = format!("{error:#}"),
                "model mutation committed but snapshot refresh failed"
            );
            let mut body = body;
            if let Some(object) = body.as_object_mut() {
                object.insert(
                    "warning".to_owned(),
                    json!(format!("saved, but applying it to the gateway failed: {error}")),
                );
            }
            (status, Json(body)).into_response()
        }
    }
}

fn bad_request(message: &str) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": message }))).into_response()
}

fn conflict(message: String) -> Response {
    (StatusCode::CONFLICT, Json(json!({ "error": message }))).into_response()
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
