# Tasks: move-config-to-database

## 1. Schema and entities

- [x] 1.1 Migration: create `providers` table (id text PK, protocol, base_url, api_key, anthropic_version nullable, model_aliases JSONB, timestamps)
- [x] 1.2 Migration: create `pricing` table (id, provider, model, rate columns as numeric, currency, pricing_source nullable, unique(provider, model))
- [x] 1.3 Sea-orm entities and repository functions for providers and pricing (list/create/update/delete, existence checks)

## 2. Snapshot from database

- [x] 2.1 Rework snapshot construction: providers + pricing overlaid from DB catalog, routes + retry from file, route→provider references validated across the boundary
- [x] 2.2 Unify rebuild triggers: boot, file watcher, `POST /reload`, and DB mutations all rebuild from cached catalog + file; keep previous-snapshot-on-failure semantics
- [x] 2.3 One-time import inside a transaction: file `[[providers]]` (with aliases) when table empty; file `[[pricing]]` when table empty; startup warning for dead file sections
- [x] 2.4 Config providers/pricing become import-only (catalog overlays them for the live snapshot)
- [x] 2.5 Tests: MemoryCatalogStore-backed conflict paths; import idempotency logic; reload overlays catalog (proxy reload tests updated)

## 3. Admin CRUD APIs

- [x] 3.1 `/admin/api/providers` GET/POST/PUT/DELETE (admin-only): validation, masked `api_key` in responses, sentinel-keeps-current on update, 409 on deleting a route-referenced provider
- [x] 3.2 Provider reveal endpoint (admin-only, single provider, 404 unknown)
- [x] 3.3 `/admin/api/pricing` GET/POST/PUT/DELETE (admin-only): non-negative decimal validation, unique provider+model conflict
- [x] 3.4 Shrink `/admin/api/config*`: structured payload reduced to routes, masked-secret set reduced to `database.url`/`admin_password`, reveal scope reduced with provider keys pointed at the new API
- [x] 3.5 Integration tests updated: routes-only editor, database.url reveal, provider/gateway_keys reveal rejected

## 4. Console UI

- [x] 4.1 Provider management view: list/create/edit/delete, masked secret with reveal/copy, referenced-delete conflict surfaced
- [x] 4.2 Pricing management view: list/create/edit/delete, provider select with `*` wildcard
- [x] 4.3 Config editor reduced to routes with "file-managed, moving to model catalog" labeling; provider options sourced from the providers API
- [x] 4.4 Navigation: add Providers and Pricing entries (admin-only)

## 5. Documentation and verification

- [x] 5.1 README: migration section (one-time import, dead sections, rollback), security note on provider keys in DB, reload/reveal notes updated
- [x] 5.2 UI builds (tsc+vite), full Rust test suite green (116 passed); live Postgres smoke test pending a running DB
