# Design: move-config-to-database

## Context

After `add-user-account-system`, users and API keys live in Postgres and the arc-swap snapshot is already rebuilt from the database on mutation. Providers, pricing, and routes still come from the TOML file through the file watcher. The admin console edits them via the structured config editor with masked sentinels and hash-guarded concurrent writes — machinery that exists only because the file is the truth.

Target state from the exploration: Postgres is the source of truth for all business configuration; the file keeps only process/bootstrap settings; the hot path reads an in-process snapshot; models become the external surface in the next change.

## Goals / Non-Goals

**Goals:**
- Providers and pricing as first-class database records with admin CRUD.
- One-time, idempotent import from the existing TOML sections.
- Snapshot rebuild on provider/pricing mutation; hot path unchanged.
- Console management views replacing the file editor for these sections.

**Non-Goals:**
- Moving `routes` (dies entirely in `add-model-catalog`; not worth a DB detour).
- Moving `retry`, `listen_addr`, `database`, `admin_password` (process/bootstrap config stays in the file).
- Encrypting provider secrets at rest (see D3).
- Model catalog, deployments, per-model pricing keys (change 3).

## Decisions

### D1: Snapshot composition becomes DB-primary, file-secondary

`GatewayState` construction changes from "parse file → build" to "read DB (providers, pricing, users, keys) + read file (routes, retry) → build". One rebuild path serves all triggers: boot, file-watcher event, `POST /reload`, or any DB mutation through the admin API. Routes from the file are validated against DB providers at build time — a route referencing a missing provider fails validation exactly as today, keeping the previous snapshot serving.

*Alternative*: separate arc-swaps per source — rejected: two snapshots can present a torn view (route → provider that just vanished); one atomic snapshot preserves today's consistency guarantee.

### D2: Import runs once, keyed on empty tables

Boot-time, in one transaction: if `providers` table is empty and the file has `[[providers]]`, import them all (including `model_aliases`, which stay as a JSON column on the provider row until change 3 relocates them); same independently for `pricing`. Afterwards the file sections are dead: parsed for the warning, never applied. No continuous sync — a one-way, one-time door. Re-import is explicit: wipe the table and restart.

*Alternative*: file-overrides-DB merge mode — rejected: two writable sources for the same record is the classic split-brain; "which one wins" questions never end.

### D3: Provider upstream keys stored plaintext in the database

The `api_key` column is plaintext, protected by DB access control, masked (`__MASKED__` + last 4) in every list/read response, revealed only via an explicit admin-only single-secret endpoint (same UX as today's file-based reveal). Encryption-at-rest with a master key is deferred: the master key would live in an env var beside the DB credentials, defending only against DB-dump-without-env leaks while adding key-rotation and ops complexity to a self-hosted gateway.

*Alternative*: envelope encryption with `LITE_AGENTIFY_MASTER_KEY` — deferred as future hardening, noted in README security notes.

### D4: The config editor shrinks instead of dying

`GET/PUT /admin/api/config*` keep working for the file's remaining fields; the structured editor UI drops its providers/pricing/gateway-keys sections and keeps routes (+ the existing masked handling for `database.url`). The whole editor is deleted in change 3 when routes die; doing the deletion there avoids UI churn twice.

### D5: CRUD endpoints follow the resource, not the file

`/admin/api/providers` (GET list, POST create, PUT update, DELETE) and `/admin/api/pricing` (same), admin-role only. Validation on write mirrors today's startup checks (non-empty id, URL with scheme+host, non-empty key, alias sanity) and additionally rejects deleting a provider that a file route still references (409 naming the route). Mutations commit to DB, then trigger snapshot rebuild; a rebuild failure after commit logs loudly but the DB write stands — next boot converges.

## Risks / Trade-offs

- [Split truth during the transition (routes in file reference providers in DB)] → Validation joins them at every build; the window closes in change 3. The editor UI labels routes as "file-managed (moving to model catalog)".
- [DB mutation + snapshot rebuild is not transactional] → Same eventual-consistency shape as change 1's key mutations; mutations are human-scale and rebuild failure is visible in logs and the admin UI (the response reports rebuild status).
- [Plaintext secrets in DB widen the blast radius of a DB compromise] → Documented; masked everywhere in APIs; reveal is single-secret, admin-only, audit-logged; encryption deferred deliberately (D3).
- [Import surprises: file edited after first boot does nothing] → Loud startup warning whenever dead sections are present; README migration section.

## Migration Plan

1. Deploy binary; on boot, empty tables + populated file sections → one-time import inside a transaction.
2. Verify providers/pricing appear in the new console views; verify proxying unchanged.
3. Delete `[[providers]]`/`[[pricing]]` from the TOML at leisure (warnings until then).
4. Rollback: previous binary reads the file sections that were never modified; DB tables are ignored by it. Any provider edits made through the API after cutover are lost on rollback — accepted, human-scale.

## Open Questions

- None blocking. Audit logging of admin mutations (who changed which provider) is a candidate follow-up once accounts exist; not required here.
