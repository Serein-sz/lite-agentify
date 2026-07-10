# Proposal: move-config-to-database

## Why

Providers and pricing are deployment-managed data that admins mutate at runtime, yet they live in a TOML file whose editing pipeline (masked sentinels, hash-guarded concurrent writes, TOML round-tripping) exists only because the file is the source of truth. With user accounts and API keys already database-backed (change `add-user-account-system`), keeping providers/pricing in the file splits the management surface across two systems. Moving them to PostgreSQL gives one source of truth, real CRUD APIs, and prepares the model catalog (`add-model-catalog`) which must reference providers relationally.

Second of four sequenced changes: `add-user-account-system` → **`move-config-to-database`** → `add-model-catalog` → `add-credit-quota`.

## What Changes

- Add `providers` and `pricing` tables in Postgres; admin CRUD APIs replace file editing for both.
- **BREAKING**: `providers` and `pricing` sections in the TOML file are no longer live configuration. On first boot with empty tables they are imported once; afterwards they are ignored with a warning.
- Provider mutations and pricing mutations trigger an in-process snapshot rebuild (same mechanism as key mutations from change 1); the request hot path still never queries the database.
- The TOML file retains only process/bootstrap concerns: `listen_addr`, `database`, `admin_password` (bootstrap seed), `retry`, and — until `add-model-catalog` — `routes`. File hot reload continues for `routes` and `retry`; routes are validated against database providers at snapshot build.
- Provider upstream API keys are stored in the database, masked in all list/read API responses, with a single-secret reveal endpoint (admin-only), replacing the file-based reveal.
- The admin console replaces the providers and pricing sections of the structured config editor with dedicated management views (provider list/create/edit/delete, pricing rule list/create/edit/delete); the config editor shrinks to routes.
- Config-hot-reload spec wording is swept to match reality after changes 1–2: gateway-key reload requirements are removed (superseded by database-backed API keys), and the reloadable set shrinks to `routes` + `retry`.

## Capabilities

### New Capabilities

- `provider-management`: database-backed provider records, admin CRUD API with masked secrets and single-secret reveal, one-time TOML import, snapshot refresh on mutation.
- `pricing-management`: database-backed pricing rules (same matching semantics as today including `*` wildcards), admin CRUD API, one-time TOML import, snapshot refresh on mutation.

### Modified Capabilities

- `admin-config`: the file-editing surface shrinks — provider/pricing fields leave the structured editor, masked-sentinel and reveal handling for provider secrets moves to `provider-management`; file config read/write still exists for the remaining file fields (routes, retry).
- `admin-ui`: the structured config editor requirement is reduced to routes; dedicated provider and pricing management views are added.
- `config-hot-reload`: reloadable file fields become `routes` and `retry` only; gateway-key reload requirements are removed; provider/pricing changes now take effect via database mutation + snapshot rebuild instead of file reload.
- `llm-gateway`: pricing for cost estimation is read from the database (wildcard fallback semantics unchanged); the "usage persistence is optional" wording is updated to the mandatory-database reality established by change 1.

## Impact

- **Code**: new migrations + `src/account/`-style entities for providers/pricing (likely `src/catalog/` or extending existing modules), `src/state.rs` (snapshot builds providers/pricing from DB), `src/reload.rs` (file reload covers fewer fields), `src/admin/config_api.rs` (editor scope shrink + new CRUD endpoints), `src/pricing/` (source moves to DB), `ui/` (new management views, editor shrink).
- **APIs**: new `/admin/api/providers` and `/admin/api/pricing` CRUD endpoints (admin-only); `/admin/api/config*` payloads lose provider/pricing sections; reveal endpoint moves/renames.
- **Migration**: one-time import of file `providers` + `pricing` on first boot with empty tables; rollback = previous binary still reads the untouched file sections.
- **Dependencies**: none new.
