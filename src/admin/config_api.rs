use std::{collections::HashMap, path::Path, time::Duration};

use anyhow::Context;
use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use toml_edit::{Array, ArrayOfTables, DocumentMut, InlineTable, Item, Table, Value};
use tracing::warn;

use super::AdminState;
use crate::{config::GatewayConfig, reload, state::GatewayState};

const MASK_PREFIX: &str = "__MASKED__";

#[derive(Serialize)]
struct ConfigPayload {
    content: String,
    hash: String,
}

pub(crate) async fn get_config(State(state): State<AdminState>) -> Response {
    let Some(path) = state.shared().config_path().map(Path::to_path_buf) else {
        return config_unavailable();
    };

    match read_masked(&path) {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => {
            warn!(error = format!("{error:#}"), "failed to read gateway config for admin console");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("failed to read config: {error}") })),
            )
                .into_response()
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct PutConfigRequest {
    content: String,
    base_hash: String,
}

pub(crate) async fn put_config(
    State(state): State<AdminState>,
    Json(request): Json<PutConfigRequest>,
) -> Response {
    let Some(path) = state.shared().config_path().map(Path::to_path_buf) else {
        return config_unavailable();
    };

    let current_bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => {
            return internal_error(format!("failed to read current config: {error}"));
        }
    };
    let current_hash = sha256_hex(&current_bytes);

    // Concurrency guard: reject writes based on a stale copy, returning the
    // fresh content so the client can reload and re-apply.
    if current_hash != request.base_hash {
        let payload = read_masked(&path).ok();
        return (
            StatusCode::CONFLICT,
            Json(json!({
                "error": "config file changed since it was loaded",
                "content": payload.as_ref().map(|payload| payload.content.clone()),
                "hash": payload.map(|payload| payload.hash),
            })),
        )
            .into_response();
    }

    let current_text = match String::from_utf8(current_bytes) {
        Ok(text) => text,
        Err(error) => return internal_error(format!("current config is not valid UTF-8: {error}")),
    };
    let current_doc = match current_text.parse::<DocumentMut>() {
        Ok(doc) => doc,
        Err(error) => return internal_error(format!("current config failed to parse: {error}")),
    };

    let mut submitted = match request.content.parse::<DocumentMut>() {
        Ok(doc) => doc,
        Err(error) => return bad_request(format!("invalid TOML: {error}")),
    };

    if let Err(field) = unmask_document(&mut submitted, &current_doc) {
        return bad_request(format!(
            "cannot resolve masked value for {field}; re-enter the real value"
        ));
    }

    let unmasked = submitted.to_string();
    let config = match toml::from_str::<GatewayConfig>(&unmasked) {
        Ok(config) => config,
        Err(error) => return bad_request(format!("invalid gateway config: {error}")),
    };

    // Validate exactly like hot reload does before anything touches the disk.
    // Top-level error message only: full chains can quote config contents.
    let current_state = state.shared().load();
    if let Err(error) = GatewayState::from_config_with_upstream_and_recorder(
        config.clone(),
        current_state.upstream.clone(),
        current_state.usage_recorder.clone(),
    ) {
        return bad_request(format!("invalid gateway config: {error}"));
    }

    let warnings = state.shared().restart_required_warnings(&config);

    if let Err(error) = atomic_write(&path, unmasked.as_bytes()) {
        warn!(error = format!("{error:#}"), "failed to persist gateway config from admin console");
        return internal_error(format!("failed to write config file: {error}"));
    }

    // Reload synchronously so the response reports the live outcome; the file
    // watcher's debounced duplicate reload of the same content is harmless.
    if let Err(error) = reload::reload(state.shared()) {
        warn!(
            error = format!("{error:#}"),
            "config written by admin console but reload failed"
        );
        return internal_error(format!("config saved but reload failed: {error}"));
    }

    Json(json!({
        "message": "configuration saved and reloaded",
        "warnings": warnings,
    }))
    .into_response()
}

/// Structured configuration exchanged with the form editor. Covers only the
/// hot-reloadable, form-editable fields; `listen_addr`, `usage_database`, and
/// `admin_password` are never touched here and survive in the on-disk document
/// via the reconcile-into-existing-document approach. Secret fields carry
/// either a real value or the `__MASKED__` sentinel (resolved on save).
#[derive(Deserialize)]
pub(crate) struct StructuredConfig {
    #[serde(default)]
    gateway_keys: Vec<String>,
    #[serde(default)]
    providers: Vec<StructuredProvider>,
    #[serde(default)]
    routes: Vec<StructuredRoute>,
    #[serde(default)]
    pricing: Vec<StructuredPricing>,
}

#[derive(Deserialize)]
struct StructuredProvider {
    id: String,
    protocol: String,
    base_url: String,
    api_key: String,
    #[serde(default)]
    anthropic_version: Option<String>,
    #[serde(default)]
    model_aliases: HashMap<String, String>,
}

#[derive(Deserialize)]
struct StructuredRoute {
    path_prefix: String,
    #[serde(default)]
    providers: Vec<String>,
    #[serde(default)]
    model_prefix: Option<String>,
}

/// Decimal fields arrive as strings so exact TOML text (e.g. `"2.00"`) is
/// preserved; `GatewayConfig` deserializes `Decimal` from these quoted values.
#[derive(Deserialize)]
struct StructuredPricing {
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

#[derive(Deserialize)]
pub(crate) struct PutStructuredRequest {
    config: StructuredConfig,
    base_hash: String,
}

pub(crate) async fn put_config_structured(
    State(state): State<AdminState>,
    Json(request): Json<PutStructuredRequest>,
) -> Response {
    let Some(path) = state.shared().config_path().map(Path::to_path_buf) else {
        return config_unavailable();
    };

    let current_bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => return internal_error(format!("failed to read current config: {error}")),
    };
    if sha256_hex(&current_bytes) != request.base_hash {
        return conflict_response(&path);
    }

    let current_text = match String::from_utf8(current_bytes) {
        Ok(text) => text,
        Err(error) => return internal_error(format!("current config is not valid UTF-8: {error}")),
    };
    let current_doc = match current_text.parse::<DocumentMut>() {
        Ok(doc) => doc,
        Err(error) => return internal_error(format!("current config failed to parse: {error}")),
    };

    // Clone the live document so non-editable fields and every comment survive,
    // then reconcile only the four editable sections into the clone.
    let mut reconciled = current_doc.clone();
    apply_structured(&mut reconciled, &request.config);

    if let Err(field) = unmask_document(&mut reconciled, &current_doc) {
        return bad_request(format!(
            "cannot resolve masked value for {field}; re-enter the real value"
        ));
    }

    let unmasked = reconciled.to_string();
    let config = match toml::from_str::<GatewayConfig>(&unmasked) {
        Ok(config) => config,
        Err(error) => return bad_request(format!("invalid gateway config: {error}")),
    };

    let current_state = state.shared().load();
    if let Err(error) = GatewayState::from_config_with_upstream_and_recorder(
        config.clone(),
        current_state.upstream.clone(),
        current_state.usage_recorder.clone(),
    ) {
        return bad_request(format!("invalid gateway config: {error}"));
    }

    let warnings = state.shared().restart_required_warnings(&config);

    if let Err(error) = atomic_write(&path, unmasked.as_bytes()) {
        warn!(error = format!("{error:#}"), "failed to persist structured gateway config");
        return internal_error(format!("failed to write config file: {error}"));
    }

    if let Err(error) = reload::reload(state.shared()) {
        warn!(
            error = format!("{error:#}"),
            "structured config written by admin console but reload failed"
        );
        return internal_error(format!("config saved but reload failed: {error}"));
    }

    Json(json!({
        "message": "configuration saved and reloaded",
        "warnings": warnings,
    }))
    .into_response()
}

/// Reconciles the structured config into the current TOML document: scalar
/// values are updated in place (their decor and same-line comments survive),
/// surviving array-of-tables entries are matched by identity and reused (their
/// comments move with them), new entries are appended fresh, and omitted ones
/// are dropped. Sections outside the form (`listen_addr`, `usage_database`,
/// `admin_password`) are never touched.
fn apply_structured(document: &mut DocumentMut, config: &StructuredConfig) {
    set_string_array(document, "gateway_keys", &config.gateway_keys);
    apply_providers(document, &config.providers);
    apply_routes(document, &config.routes);
    apply_pricing(document, &config.pricing);
}

fn apply_providers(document: &mut DocumentMut, providers: &[StructuredProvider]) {
    let mut existing = take_tables(document, "providers");
    let mut result = ArrayOfTables::new();
    for provider in providers {
        let mut table = take_matching(&mut existing, |table| {
            table.get("id").and_then(Item::as_str) == Some(provider.id.as_str())
        })
        .unwrap_or_default();
        set_string(&mut table, "id", &provider.id);
        set_string(&mut table, "protocol", &provider.protocol);
        set_string(&mut table, "base_url", &provider.base_url);
        set_string(&mut table, "api_key", &provider.api_key);
        set_optional_string(
            &mut table,
            "anthropic_version",
            provider.anthropic_version.as_deref(),
        );
        set_string_map(&mut table, "model_aliases", &provider.model_aliases);
        result.push(table);
    }
    set_array_of_tables(document, "providers", result);
}

fn apply_routes(document: &mut DocumentMut, routes: &[StructuredRoute]) {
    let mut existing = take_tables(document, "routes");
    let mut result = ArrayOfTables::new();
    for route in routes {
        let mut table = take_matching(&mut existing, |table| {
            table.get("path_prefix").and_then(Item::as_str) == Some(route.path_prefix.as_str())
        })
        .unwrap_or_default();
        set_string(&mut table, "path_prefix", &route.path_prefix);
        set_string_array(&mut table, "providers", &route.providers);
        set_optional_string(&mut table, "model_prefix", route.model_prefix.as_deref());
        result.push(table);
    }
    set_array_of_tables(document, "routes", result);
}

fn apply_pricing(document: &mut DocumentMut, pricing: &[StructuredPricing]) {
    let mut existing = take_tables(document, "pricing");
    let mut result = ArrayOfTables::new();
    for entry in pricing {
        let mut table = take_matching(&mut existing, |table| {
            table.get("provider").and_then(Item::as_str) == Some(entry.provider.as_str())
                && table.get("model").and_then(Item::as_str) == Some(entry.model.as_str())
        })
        .unwrap_or_default();
        set_string(&mut table, "provider", &entry.provider);
        set_string(&mut table, "model", &entry.model);
        set_string(&mut table, "input_per_1m", &entry.input_per_1m);
        set_string(&mut table, "output_per_1m", &entry.output_per_1m);
        set_optional_string(
            &mut table,
            "cached_input_per_1m",
            entry.cached_input_per_1m.as_deref(),
        );
        set_optional_string(
            &mut table,
            "cache_read_per_1m",
            entry.cache_read_per_1m.as_deref(),
        );
        set_optional_string(
            &mut table,
            "cache_write_per_1m",
            entry.cache_write_per_1m.as_deref(),
        );
        set_string(&mut table, "currency", &entry.currency);
        set_optional_string(&mut table, "pricing_source", entry.pricing_source.as_deref());
        result.push(table);
    }
    set_array_of_tables(document, "pricing", result);
}

/// Takes the existing array-of-tables entries under `key` out of the document,
/// converting inline-array entries to tables so both storage styles reconcile
/// the same way. Reused tables keep their doc position and comments.
fn take_tables(document: &mut DocumentMut, key: &str) -> Vec<Table> {
    match document.remove(key) {
        Some(Item::ArrayOfTables(tables)) => tables.into_iter().collect(),
        Some(Item::Value(Value::Array(array))) => array
            .into_iter()
            .filter_map(|value| match value {
                Value::InlineTable(inline) => Some(inline.into_table()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn take_matching(tables: &mut Vec<Table>, matches: impl Fn(&Table) -> bool) -> Option<Table> {
    let index = tables.iter().position(matches)?;
    Some(tables.remove(index))
}

fn set_array_of_tables(document: &mut DocumentMut, key: &str, tables: ArrayOfTables) {
    if tables.is_empty() {
        document.remove(key);
    } else {
        document.insert(key, Item::ArrayOfTables(tables));
    }
}

/// Sets a string entry, leaving the node untouched when the value is unchanged
/// and keeping its decor (same-line comments) when replacing.
fn set_string(table: &mut Table, key: &str, new_value: &str) {
    match table.get_mut(key).and_then(Item::as_value_mut) {
        Some(value) if value.as_str() == Some(new_value) => {}
        Some(value) => replace_string(value, new_value.to_owned()),
        None => {
            table.insert(key, Item::Value(Value::from(new_value)));
        }
    }
}

fn set_optional_string(table: &mut Table, key: &str, new_value: Option<&str>) {
    match new_value {
        Some(value) => set_string(table, key, value),
        None => {
            table.remove(key);
        }
    }
}

/// Updates a string array in place: surviving positions keep their decor, the
/// array grows or shrinks to match, and the item is created when missing.
fn set_string_array(table: &mut Table, key: &str, values: &[String]) {
    let item = table
        .entry(key)
        .or_insert_with(|| Item::Value(Value::Array(Array::new())));
    if item.as_array().is_none() {
        *item = Item::Value(Value::Array(Array::new()));
    }
    let array = item.as_array_mut().expect("ensured above");
    while array.len() > values.len() {
        array.remove(array.len() - 1);
    }
    for (index, value) in values.iter().enumerate() {
        match array.get_mut(index) {
            Some(existing) if existing.as_str() == Some(value) => {}
            Some(existing) => replace_string(existing, value.clone()),
            None => array.push(value.as_str()),
        }
    }
}

/// Updates a key/value string map (e.g. `model_aliases`): existing entries keep
/// their decor, stale keys are removed, new keys are inserted sorted for
/// deterministic output, and an empty map removes the item entirely.
fn set_string_map(table: &mut Table, key: &str, map: &HashMap<String, String>) {
    if map.is_empty() {
        table.remove(key);
        return;
    }
    if table.get(key).and_then(Item::as_table_like).is_none() {
        table.insert(key, Item::Value(Value::InlineTable(InlineTable::new())));
    }
    let target = table
        .get_mut(key)
        .and_then(Item::as_table_like_mut)
        .expect("ensured above");
    let stale: Vec<String> = target
        .iter()
        .map(|(entry_key, _)| entry_key.to_owned())
        .filter(|entry_key| !map.contains_key(entry_key))
        .collect();
    for entry_key in stale {
        target.remove(&entry_key);
    }
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort();
    for entry_key in keys {
        let new_value = map[entry_key].as_str();
        match target.get_mut(entry_key).and_then(Item::as_value_mut) {
            Some(value) if value.as_str() == Some(new_value) => {}
            Some(value) => replace_string(value, new_value.to_owned()),
            None => {
                target.insert(entry_key, Item::Value(Value::from(new_value)));
            }
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct RevealRequest {
    field: String,
}

/// Returns one named secret's plaintext from the on-disk config. Session-gated
/// by the router middleware. Field references: `providers.<id>.api_key`,
/// `usage_database.url`, `gateway_keys.<index>`. Non-secret references are
/// `400`; secret-shaped references that resolve to nothing are `404`.
pub(crate) async fn reveal_secret(
    State(state): State<AdminState>,
    Json(request): Json<RevealRequest>,
) -> Response {
    let Some(path) = state.shared().config_path().map(Path::to_path_buf) else {
        return config_unavailable();
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) => return internal_error(format!("failed to read config: {error}")),
    };
    let document = match text.parse::<DocumentMut>() {
        Ok(document) => document,
        Err(error) => return internal_error(format!("config failed to parse: {error}")),
    };

    let field = request.field.as_str();
    let value: Option<String> = if field == "usage_database.url" {
        document
            .get("usage_database")
            .and_then(Item::as_table_like)
            .and_then(|table| table.get("url"))
            .and_then(Item::as_str)
            .map(str::to_owned)
    } else if let Some(index) = field.strip_prefix("gateway_keys.") {
        let Ok(index) = index.parse::<usize>() else {
            return not_found("unknown field reference");
        };
        document
            .get("gateway_keys")
            .and_then(Item::as_array)
            .and_then(|array| array.get(index))
            .and_then(Value::as_str)
            .map(str::to_owned)
    } else if let Some(rest) = field.strip_prefix("providers.") {
        let Some((id, sub)) = rest.rsplit_once('.') else {
            return bad_request("not a secret field".to_owned());
        };
        if sub != "api_key" {
            return bad_request("not a secret field".to_owned());
        }
        provider_api_key(&document, id)
    } else {
        return bad_request("not a secret field".to_owned());
    };

    match value {
        Some(value) => Json(json!({ "value": value })).into_response(),
        None => not_found("unknown field reference"),
    }
}

fn provider_api_key(document: &DocumentMut, id: &str) -> Option<String> {
    let mut found = None;
    for_each_provider_value(document, |provider_id, key, value| {
        if key == "api_key"
            && provider_id == Some(id)
            && let Some(secret) = value.as_str()
        {
            found = Some(secret.to_owned());
        }
    });
    found
}

fn config_unavailable() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": "config editing is unavailable: hot reload is not configured" })),
    )
        .into_response()
}

fn not_found(error: &str) -> Response {
    (StatusCode::NOT_FOUND, Json(json!({ "error": error }))).into_response()
}

/// Builds the `409` response carrying the current masked content and hash so
/// the client can reload and re-apply after a concurrent modification.
fn conflict_response(path: &Path) -> Response {
    let payload = read_masked(path).ok();
    (
        StatusCode::CONFLICT,
        Json(json!({
            "error": "config file changed since it was loaded",
            "content": payload.as_ref().map(|payload| payload.content.clone()),
            "hash": payload.map(|payload| payload.hash),
        })),
    )
        .into_response()
}

fn bad_request(error: String) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))).into_response()
}

fn internal_error(error: String) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": error })),
    )
        .into_response()
}

fn read_masked(path: &Path) -> anyhow::Result<ConfigPayload> {
    let bytes = std::fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    let hash = sha256_hex(&bytes);
    let text = String::from_utf8(bytes).context("config file is not valid UTF-8")?;
    let mut document = text
        .parse::<DocumentMut>()
        .context("config file failed to parse as TOML")?;
    mask_document(&mut document);
    Ok(ConfigPayload {
        content: document.to_string(),
        hash,
    })
}

fn is_masked(value: &str) -> bool {
    value.starts_with(MASK_PREFIX)
}

/// Masks retaining the last 4 characters for identification; short secrets are
/// masked entirely so most of their material is never revealed.
fn mask_keeping_suffix(secret: &str) -> String {
    let chars = secret.chars().count();
    if chars >= 8 {
        let suffix: String = secret.chars().skip(chars - 4).collect();
        format!("{MASK_PREFIX}{suffix}")
    } else {
        MASK_PREFIX.to_owned()
    }
}

/// Replaces a string value in place, keeping its surrounding whitespace and
/// same-line comments intact.
fn replace_string(value: &mut Value, replacement: String) {
    let decor = value.decor().clone();
    *value = Value::from(replacement);
    *value.decor_mut() = decor;
}

fn mask_document(document: &mut DocumentMut) {
    // Fully masked: their values must never leak, even partially.
    if let Some(value) = document
        .get_mut("admin_password")
        .and_then(Item::as_value_mut)
        && value.is_str()
    {
        replace_string(value, MASK_PREFIX.to_owned());
    }
    if let Some(value) = document
        .get_mut("usage_database")
        .and_then(|item| item.as_table_like_mut())
        .and_then(|table| table.get_mut("url"))
        .and_then(Item::as_value_mut)
        && value.is_str()
    {
        replace_string(value, MASK_PREFIX.to_owned());
    }

    if let Some(array) = document.get_mut("gateway_keys").and_then(Item::as_array_mut) {
        for value in array.iter_mut() {
            if let Some(secret) = value.as_str() {
                let masked = mask_keeping_suffix(secret);
                replace_string(value, masked);
            }
        }
    }

    for_each_provider_value_mut(document, "api_key", &mut |value| {
        if let Some(secret) = value.as_str() {
            let masked = mask_keeping_suffix(secret);
            replace_string(value, masked);
        }
    });
}

/// Restores `__MASKED__` sentinels from the current on-disk document. Returns
/// the offending field name when a sentinel cannot be resolved unambiguously.
fn unmask_document(submitted: &mut DocumentMut, current: &DocumentMut) -> Result<(), String> {
    // admin_password / usage_database.url: positional.
    if let Some(value) = submitted
        .get_mut("admin_password")
        .and_then(Item::as_value_mut)
        && value.as_str().is_some_and(is_masked)
    {
        let real = current
            .get("admin_password")
            .and_then(Item::as_str)
            .ok_or("admin_password")?;
        replace_string(value, real.to_owned());
    }

    if let Some(value) = submitted
        .get_mut("usage_database")
        .and_then(|item| item.as_table_like_mut())
        .and_then(|table| table.get_mut("url"))
        .and_then(Item::as_value_mut)
        && value.as_str().is_some_and(is_masked)
    {
        let real = current
            .get("usage_database")
            .and_then(|item| item.as_table_like())
            .and_then(|table| table.get("url"))
            .and_then(Item::as_str)
            .ok_or("usage_database.url")?;
        replace_string(value, real.to_owned());
    }

    // gateway_keys: by index, and only when the list length is unchanged —
    // otherwise a sentinel's position is ambiguous.
    if let Some(array) = submitted
        .get_mut("gateway_keys")
        .and_then(Item::as_array_mut)
        && array.iter().any(|value| value.as_str().is_some_and(is_masked))
    {
        let current_keys: Vec<Option<String>> = current
            .get("gateway_keys")
            .and_then(Item::as_array)
            .map(|array| {
                array
                    .iter()
                    .map(|value| value.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        if current_keys.len() != array.len() {
            return Err(
                "gateway_keys (list length changed; re-enter real values instead of masked ones)"
                    .to_owned(),
            );
        }
        for (index, value) in array.iter_mut().enumerate() {
            if value.as_str().is_some_and(is_masked) {
                let real = current_keys
                    .get(index)
                    .and_then(Option::as_deref)
                    .ok_or(format!("gateway_keys[{index}]"))?;
                replace_string(value, real.to_owned());
            }
        }
    }

    // providers[].api_key: matched by provider id.
    let mut current_api_keys: HashMap<String, String> = HashMap::new();
    for_each_provider_value(current, |id, key, value| {
        if key == "api_key"
            && let (Some(id), Some(secret)) = (id, value.as_str())
        {
            current_api_keys.insert(id.to_owned(), secret.to_owned());
        }
    });

    let mut unresolved: Option<String> = None;
    for_each_provider_table_mut(submitted, &mut |table| {
        let id = table
            .get("id")
            .and_then(Item::as_str)
            .map(str::to_owned);
        let Some(value) = table.get_mut("api_key").and_then(Item::as_value_mut) else {
            return;
        };
        if !value.as_str().is_some_and(is_masked) {
            return;
        }
        let real = id
            .as_deref()
            .and_then(|id| current_api_keys.get(id).cloned());
        match real {
            Some(real) => replace_string(value, real),
            None => {
                if unresolved.is_none() {
                    unresolved = Some(match id {
                        Some(id) => format!("providers[id={id}].api_key"),
                        None => "providers[].api_key (provider has no id)".to_owned(),
                    });
                }
            }
        }
    });
    if let Some(field) = unresolved {
        return Err(field);
    }

    Ok(())
}

/// Visits every provider entry's `key` value mutably, whether providers are
/// written as `[[providers]]` tables or as an inline array of tables.
fn for_each_provider_value_mut(
    document: &mut DocumentMut,
    key: &str,
    visit: &mut dyn FnMut(&mut Value),
) {
    for_each_provider_table_mut(document, &mut |table| {
        if let Some(value) = table.get_mut(key).and_then(Item::as_value_mut) {
            visit(value);
        }
    });
}

fn for_each_provider_table_mut(
    document: &mut DocumentMut,
    visit: &mut dyn FnMut(&mut dyn toml_edit::TableLike),
) {
    match document.get_mut("providers") {
        Some(Item::ArrayOfTables(tables)) => {
            for table in tables.iter_mut() {
                visit(table);
            }
        }
        Some(Item::Value(Value::Array(array))) => {
            for value in array.iter_mut() {
                if let Some(table) = value.as_inline_table_mut() {
                    visit(table);
                }
            }
        }
        _ => {}
    }
}

/// Visits every provider entry's values immutably as (provider id, key, value).
fn for_each_provider_value(document: &DocumentMut, mut visit: impl FnMut(Option<&str>, &str, &Value)) {
    let visit_table = |table: &dyn toml_edit::TableLike, visit: &mut dyn FnMut(Option<&str>, &str, &Value)| {
        let id = table.get("id").and_then(Item::as_str);
        for (key, item) in table.iter() {
            if let Some(value) = item.as_value() {
                visit(id, key, value);
            }
        }
    };
    match document.get("providers") {
        Some(Item::ArrayOfTables(tables)) => {
            for table in tables.iter() {
                visit_table(table, &mut visit);
            }
        }
        Some(Item::Value(Value::Array(array))) => {
            for value in array.iter() {
                if let Some(table) = value.as_inline_table() {
                    visit_table(table, &mut visit);
                }
            }
        }
        _ => {}
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write;

    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

/// Writes via a temp file in the same directory plus rename, retrying once:
/// on Windows a concurrently open editor can briefly hold the destination.
fn atomic_write(path: &Path, contents: &[u8]) -> anyhow::Result<()> {
    let temp = path.with_extension("toml.tmp");
    std::fs::write(&temp, contents)
        .with_context(|| format!("failed to write {}", temp.display()))?;
    if std::fs::rename(&temp, path).is_err() {
        std::thread::sleep(Duration::from_millis(100));
        if let Err(error) = std::fs::rename(&temp, path) {
            let _ = std::fs::remove_file(&temp);
            return Err(error).with_context(|| {
                format!("failed to replace {} atomically", path.display())
            });
        }
    }
    Ok(())
}
