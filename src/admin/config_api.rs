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
use toml_edit::{DocumentMut, Item, Value};
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

fn config_unavailable() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": "config editing is unavailable: hot reload is not configured" })),
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
