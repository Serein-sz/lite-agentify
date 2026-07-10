use std::collections::HashMap;

use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use super::{AdminState, SessionIdentity, require_admin};
use crate::{
    catalog::CatalogConflict,
    config::{PricingConfig, ProviderConfig},
    model::Protocol,
};

/// Prefix marking a masked secret in API responses; a submitted value starting
/// with it means "keep the current secret".
pub(super) const MASK_PREFIX: &str = "__MASKED__";

/// Masks retaining the last 4 characters for identification; short secrets are
/// masked entirely so most of their material is never revealed.
pub(super) fn mask_keeping_suffix(secret: &str) -> String {
    let chars = secret.chars().count();
    if chars >= 8 {
        let suffix: String = secret.chars().skip(chars - 4).collect();
        format!("{MASK_PREFIX}{suffix}")
    } else {
        MASK_PREFIX.to_owned()
    }
}

// --- providers ---

fn provider_json(provider: &ProviderConfig) -> serde_json::Value {
    json!({
        "id": provider.id,
        "protocol": provider.protocol.to_string(),
        "base_url": provider.base_url,
        "api_key": mask_keeping_suffix(&provider.api_key),
        "anthropic_version": provider.anthropic_version,
        "model_aliases": provider.model_aliases,
    })
}

pub(crate) async fn list_providers(
    State(state): State<AdminState>,
    Extension(identity): Extension<SessionIdentity>,
) -> Response {
    if let Err(response) = require_admin(&identity) {
        return response;
    }
    match state.catalog().list_providers().await {
        Ok(providers) => {
            let rows: Vec<_> = providers.iter().map(provider_json).collect();
            Json(json!({ "providers": rows })).into_response()
        }
        Err(error) => internal(error, "failed to list providers"),
    }
}

#[derive(Deserialize)]
pub(crate) struct ProviderBody {
    protocol: String,
    base_url: String,
    api_key: String,
    #[serde(default)]
    anthropic_version: Option<String>,
    #[serde(default)]
    model_aliases: HashMap<String, String>,
}

fn parse_protocol(value: &str) -> Option<Protocol> {
    match value {
        "openai" | "open-ai" => Some(Protocol::OpenAi),
        "anthropic" => Some(Protocol::Anthropic),
        _ => None,
    }
}

fn build_provider(id: String, body: ProviderBody, current_key: Option<&str>) -> Result<ProviderConfig, Response> {
    let Some(protocol) = parse_protocol(&body.protocol) else {
        return Err(bad_request("protocol must be 'openai' or 'anthropic'"));
    };
    if body.base_url.trim().is_empty() {
        return Err(bad_request("base_url cannot be empty"));
    }
    // A masked api_key means "keep the current value" on update.
    let api_key = if body.api_key.starts_with(MASK_PREFIX) {
        match current_key {
            Some(key) => key.to_owned(),
            None => return Err(bad_request("api_key is required")),
        }
    } else {
        body.api_key
    };
    if api_key.trim().is_empty() {
        return Err(bad_request("api_key cannot be empty"));
    }
    Ok(ProviderConfig {
        id,
        protocol,
        base_url: body.base_url,
        api_key,
        anthropic_version: body.anthropic_version,
        model_aliases: body.model_aliases,
    })
}

#[derive(Deserialize)]
pub(crate) struct CreateProviderBody {
    id: String,
    #[serde(flatten)]
    fields: ProviderBody,
}

pub(crate) async fn create_provider(
    State(state): State<AdminState>,
    Extension(identity): Extension<SessionIdentity>,
    Json(body): Json<CreateProviderBody>,
) -> Response {
    if let Err(response) = require_admin(&identity) {
        return response;
    }
    let id = body.id.trim().to_owned();
    if id.is_empty() {
        return bad_request("provider id cannot be empty");
    }
    let provider = match build_provider(id, body.fields, None) {
        Ok(provider) => provider,
        Err(response) => return response,
    };
    match state.catalog().upsert_provider(provider, true).await {
        Ok(()) => refresh_and_respond(&state, StatusCode::CREATED, json!({ "ok": true })).await,
        Err(error) if error.is::<CatalogConflict>() => conflict(error.to_string()),
        Err(error) => internal(error, "failed to create provider"),
    }
}

pub(crate) async fn update_provider(
    State(state): State<AdminState>,
    Extension(identity): Extension<SessionIdentity>,
    Path(id): Path<String>,
    Json(fields): Json<ProviderBody>,
) -> Response {
    if let Err(response) = require_admin(&identity) {
        return response;
    }
    let current = match state.catalog().get_provider(&id).await {
        Ok(Some(provider)) => provider,
        Ok(None) => return not_found("provider not found"),
        Err(error) => return internal(error, "failed to load provider"),
    };
    let provider = match build_provider(id, fields, Some(&current.api_key)) {
        Ok(provider) => provider,
        Err(response) => return response,
    };
    match state.catalog().upsert_provider(provider, false).await {
        Ok(()) => refresh_and_respond(&state, StatusCode::OK, json!({ "ok": true })).await,
        Err(error) => internal(error, "failed to update provider"),
    }
}

pub(crate) async fn delete_provider(
    State(state): State<AdminState>,
    Extension(identity): Extension<SessionIdentity>,
    Path(id): Path<String>,
) -> Response {
    if let Err(response) = require_admin(&identity) {
        return response;
    }
    // A model deployment may still reference this provider; deleting it would
    // make the next snapshot rebuild fail. Reject with 409 naming the model.
    match state.catalog().provider_in_use(&id).await {
        Ok(Some(model)) => {
            return conflict(format!(
                "provider '{id}' is still used by model '{model}'; remove that deployment first"
            ));
        }
        Ok(None) => {}
        Err(error) => return internal(error, "failed to check provider usage"),
    }
    match state.catalog().delete_provider(&id).await {
        Ok(true) => refresh_and_respond(&state, StatusCode::OK, json!({ "ok": true })).await,
        Ok(false) => not_found("provider not found"),
        Err(error) => internal(error, "failed to delete provider"),
    }
}

pub(crate) async fn reveal_provider_key(
    State(state): State<AdminState>,
    Extension(identity): Extension<SessionIdentity>,
    Path(id): Path<String>,
) -> Response {
    if let Err(response) = require_admin(&identity) {
        return response;
    }
    match state.catalog().get_provider(&id).await {
        Ok(Some(provider)) => Json(json!({ "value": provider.api_key })).into_response(),
        Ok(None) => not_found("provider not found"),
        Err(error) => internal(error, "failed to load provider"),
    }
}

// --- pricing ---

fn pricing_json(id: Uuid, config: &PricingConfig) -> serde_json::Value {
    json!({
        "id": id,
        "provider": config.provider,
        "model": config.model,
        "input_per_1m": config.input_per_1m.to_string(),
        "output_per_1m": config.output_per_1m.to_string(),
        "cached_input_per_1m": config.cached_input_per_1m.map(|d| d.to_string()),
        "cache_read_per_1m": config.cache_read_per_1m.map(|d| d.to_string()),
        "cache_write_per_1m": config.cache_write_per_1m.map(|d| d.to_string()),
        "currency": config.currency,
        "pricing_source": config.pricing_source,
    })
}

pub(crate) async fn list_pricing(
    State(state): State<AdminState>,
    Extension(identity): Extension<SessionIdentity>,
) -> Response {
    if let Err(response) = require_admin(&identity) {
        return response;
    }
    match state.catalog().list_pricing().await {
        Ok(records) => {
            let rows: Vec<_> = records
                .iter()
                .map(|record| pricing_json(record.id, &record.config))
                .collect();
            Json(json!({ "pricing": rows })).into_response()
        }
        Err(error) => internal(error, "failed to list pricing"),
    }
}

#[derive(Deserialize)]
pub(crate) struct PricingBody {
    provider: String,
    model: String,
    input_per_1m: String,
    output_per_1m: String,
    #[serde(default)]
    cached_input_per_1m: Option<String>,
    #[serde(default)]
    cache_read_per_1m: Option<String>,
    #[serde(default)]
    cache_write_per_1m: Option<String>,
    currency: String,
    #[serde(default)]
    pricing_source: Option<String>,
}

fn parse_rate(name: &str, value: &str) -> Result<Decimal, Response> {
    let decimal: Decimal = value
        .parse()
        .map_err(|_| bad_request(&format!("{name} must be a decimal number")))?;
    if decimal.is_sign_negative() {
        return Err(bad_request(&format!("{name} cannot be negative")));
    }
    Ok(decimal)
}

fn parse_optional_rate(name: &str, value: Option<&str>) -> Result<Option<Decimal>, Response> {
    match value {
        None => Ok(None),
        Some(value) if value.trim().is_empty() => Ok(None),
        Some(value) => Ok(Some(parse_rate(name, value)?)),
    }
}

fn build_pricing(body: PricingBody) -> Result<PricingConfig, Response> {
    if body.provider.trim().is_empty() {
        return Err(bad_request("provider cannot be empty"));
    }
    if body.model.trim().is_empty() {
        return Err(bad_request("model cannot be empty"));
    }
    let currency = body.currency.trim().to_owned();
    if currency.len() != 3 || !currency.chars().all(|c| c.is_ascii_uppercase()) {
        return Err(bad_request("currency must be a three-letter uppercase ISO code"));
    }
    Ok(PricingConfig {
        provider: body.provider,
        model: body.model,
        input_per_1m: parse_rate("input_per_1m", &body.input_per_1m)?,
        output_per_1m: parse_rate("output_per_1m", &body.output_per_1m)?,
        cached_input_per_1m: parse_optional_rate(
            "cached_input_per_1m",
            body.cached_input_per_1m.as_deref(),
        )?,
        cache_read_per_1m: parse_optional_rate(
            "cache_read_per_1m",
            body.cache_read_per_1m.as_deref(),
        )?,
        cache_write_per_1m: parse_optional_rate(
            "cache_write_per_1m",
            body.cache_write_per_1m.as_deref(),
        )?,
        currency,
        pricing_source: body.pricing_source,
    })
}

pub(crate) async fn create_pricing(
    State(state): State<AdminState>,
    Extension(identity): Extension<SessionIdentity>,
    Json(body): Json<PricingBody>,
) -> Response {
    if let Err(response) = require_admin(&identity) {
        return response;
    }
    let config = match build_pricing(body) {
        Ok(config) => config,
        Err(response) => return response,
    };
    match state.catalog().create_pricing(config).await {
        Ok(_) => refresh_and_respond(&state, StatusCode::CREATED, json!({ "ok": true })).await,
        Err(error) if error.is::<CatalogConflict>() => conflict(error.to_string()),
        Err(error) => internal(error, "failed to create pricing"),
    }
}

pub(crate) async fn update_pricing(
    State(state): State<AdminState>,
    Extension(identity): Extension<SessionIdentity>,
    Path(id): Path<Uuid>,
    Json(body): Json<PricingBody>,
) -> Response {
    if let Err(response) = require_admin(&identity) {
        return response;
    }
    let config = match build_pricing(body) {
        Ok(config) => config,
        Err(response) => return response,
    };
    // The listing gate's invariant — every deployment of an enabled model has
    // pricing coverage — must survive this edit (a changed provider/model pair
    // can strip coverage just like a delete).
    if let Err(response) =
        ensure_pricing_gate(&state, |rules| {
            rules.retain(|record| record.id != id);
            rules.push(crate::catalog::PricingRecord {
                id,
                config: config.clone(),
            });
        })
        .await
    {
        return response;
    }
    match state.catalog().update_pricing(id, config).await {
        Ok(true) => refresh_and_respond(&state, StatusCode::OK, json!({ "ok": true })).await,
        Ok(false) => not_found("pricing rule not found"),
        Err(error) if error.is::<CatalogConflict>() => conflict(error.to_string()),
        Err(error) => internal(error, "failed to update pricing"),
    }
}

pub(crate) async fn delete_pricing(
    State(state): State<AdminState>,
    Extension(identity): Extension<SessionIdentity>,
    Path(id): Path<Uuid>,
) -> Response {
    if let Err(response) = require_admin(&identity) {
        return response;
    }
    if let Err(response) =
        ensure_pricing_gate(&state, |rules| rules.retain(|record| record.id != id)).await
    {
        return response;
    }
    match state.catalog().delete_pricing(id).await {
        Ok(true) => refresh_and_respond(&state, StatusCode::OK, json!({ "ok": true })).await,
        Ok(false) => not_found("pricing rule not found"),
        Err(error) => internal(error, "failed to delete pricing"),
    }
}

/// Simulates a pricing-rule mutation and rejects it (409, naming the model and
/// deployment) when any **enabled** model would lose pricing coverage.
/// Disabled models are exempt — they are drafts.
async fn ensure_pricing_gate(
    state: &AdminState,
    mutate: impl FnOnce(&mut Vec<crate::catalog::PricingRecord>),
) -> Result<(), Response> {
    let mut rules = state
        .catalog()
        .list_pricing()
        .await
        .map_err(|error| internal(error, "failed to load pricing"))?;
    mutate(&mut rules);
    let candidate: Vec<PricingConfig> = rules.into_iter().map(|record| record.config).collect();

    let models = state
        .catalog()
        .list_models()
        .await
        .map_err(|error| internal(error, "failed to load models"))?;
    for model in models.iter().filter(|model| model.enabled) {
        if let Some(deployment) =
            crate::catalog::uncovered_deployments(model, &candidate).first()
        {
            return Err(conflict(format!(
                "this pricing change would leave enabled model '{}' without coverage for \
                 deployment '{}:{}'; disable the model or add a replacement rule first",
                model.name, deployment.provider_id, deployment.upstream_model
            )));
        }
    }
    Ok(())
}

// --- helpers ---

/// Refreshes the gateway snapshot after a catalog mutation. A refresh failure
/// (e.g. the new catalog fails validation) is reported so the admin sees it;
/// the database write has already committed.
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
                "catalog mutation committed but snapshot refresh failed"
            );
            let mut body = body;
            if let Some(object) = body.as_object_mut() {
                object.insert(
                    "warning".to_owned(),
                    json!(format!(
                        "saved, but applying it to the gateway failed: {error}"
                    )),
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
