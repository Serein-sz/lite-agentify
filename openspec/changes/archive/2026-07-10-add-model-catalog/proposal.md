# Proposal: add-model-catalog

## Why

Routing is still path-and-provider-shaped: clients must know which path prefix maps to which provider chain, and per-provider `model_aliases` awkwardly encode "what this provider calls that model" from the provider's side. With accounts and database-backed providers in place, the agreed target surface is: **clients see only models**. Admins curate a model catalog where each model carries an ordered deployment chain (provider + that provider's upstream model name); routing, failover, and name translation become catalog lookups. This also creates the anchor that API key permissions (`allowed_models`) and per-model spend need.

Third of four sequenced changes: `add-user-account-system` → `move-config-to-database` → **`add-model-catalog`** → `add-credit-quota`.

## What Changes

- Add `models` and `model_deployments` tables: a model has a public name, enabled/disabled status, and an ordered list of deployments (provider id + upstream model name); order is failover priority.
- **BREAKING**: request routing keys on the request body's `model` against the catalog instead of path-prefix route rules. A model not in the catalog (or disabled) is rejected — pass-through of unknown models ends.
- **BREAKING**: `routes` config and per-provider `model_aliases` are removed; a one-time migration converts existing route chains + aliases into catalog entries where derivable, logging what needs manual attention.
- Endpoint path determines protocol only (`/v1/chat/completions`, `/v1/responses` = OpenAI-style; `/v1/messages` = Anthropic-style); at resolution the model's deployment chain is filtered to providers matching the endpoint protocol (no protocol translation — exploration decision A). Same failover and retry semantics as today along the filtered chain, with per-deployment upstream model rewriting.
- Enabling a model requires pricing coverage for every deployment (via the existing wildcard fallback); deleting pricing that would strip coverage from an enabled model is rejected.
- `GET /v1/models` becomes gateway-owned: it returns the cataloged models available to the presented key in protocol-native shape, instead of forwarding upstream.
- API keys gain optional `allowed_models`; absent means all cataloged models. Enforcement happens before any upstream contact.
- Console: new admin model-catalog management view; key creation gains model selection; the structured config editor is removed (no form-editable file fields remain — the file is down to `listen_addr`, `database`, `admin_password`, `retry`).

## Capabilities

### New Capabilities

- `model-catalog`: catalog and deployment storage, model-based request resolution with protocol filtering and upstream-name rewriting, pricing-coverage gate, gateway-owned `/v1/models`, admin CRUD, one-time migration from routes/aliases.

### Modified Capabilities

- `llm-gateway`: path-prefix routing, startup chain validation, and the three model-alias requirements are removed/replaced by catalog-based resolution; failover chain requirement is restated over deployment chains; protocol non-conversion is restated over catalog resolution.
- `api-key-management`: keys gain `allowed_models` restriction enforced before upstream contact.
- `admin-config`: the structured form write requirements are removed (no form-editable fields remain; the file is hand-edited process/bootstrap config).
- `config-hot-reload`: file-reloadable fields shrink to `retry`.
- `admin-ui`: the config editor requirement is removed; a model catalog management view is added; key creation offers model selection.

## Impact

- **Code**: `src/proxy/router.rs` (model-based resolution), `src/state.rs` (catalog in snapshot), `src/model.rs` (Route removal), migrations + entities for models/deployments, `src/admin/` (catalog CRUD, `/v1/models` handler, key allowed_models), `ui/` (catalog view, key model-picker, editor removal), README.
- **APIs**: new `/admin/api/models` CRUD; `GET /v1/models` behavior change; proxied requests without a body `model` field (e.g. some GET endpoints) need explicit handling (rejected except gateway-owned paths).
- **Migration**: one-time conversion of file routes+aliases into catalog entries (alias-bearing chains convert cleanly; pass-through models cannot be enumerated and are logged for manual creation); `routes` section becomes dead with a warning.
- **Clients**: must send a cataloged model name; unknown models now fail fast with a clear error.
