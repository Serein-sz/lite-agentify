# Tasks: add-model-catalog

## 1. Schema and entities

- [x] 1.1 Migration: create `models` (name PK, status, created_at) and `model_deployments` (id, model_name FK, provider_id FK, upstream_model, priority, unique(model_name, provider_id))
- [x] 1.2 Migration: add nullable `allowed_models` JSONB to `api_keys`
- [x] 1.3 Entities and repository functions: catalog CRUD, deployment reorder, pricing-coverage query (deployment → rule via wildcard fallback)

## 2. Resolution engine

- [x] 2.1 Snapshot: catalog map (model → status + ordered deployments), key entries carry allowed_models set
- [x] 2.2 Replace `match_route` with resolve(protocol, model, key): unknown/disabled model error, allowed_models 403, protocol filter, clear no-matching-protocol error — all before upstream contact
- [x] 2.3 Failover walk over filtered deployments with per-attempt upstream model rewrite (reuse existing failover/retry/streaming-decision machinery); remove Route/model_prefix from `src/model.rs` and router
- [x] 2.4 Fixed protocol paths: `/v1/chat/completions`, `/v1/responses` (OpenAI), `/v1/messages` (Anthropic); non-model endpoints no longer proxied except gateway-owned paths
- [x] 2.5 Gateway-owned `GET /v1/models`: enabled models scoped to the key, protocol-native shape per endpoint family
- [x] 2.6 Tests: resolution matrix (unknown/disabled/403/protocol-filter/order), rewrite correctness, /v1/models scoping

## 3. Pricing gate

- [x] 3.1 Enforce coverage on: model enable, deployment mutation of enabled model, pricing delete/update stripping coverage (409 naming model/deployment)
- [x] 3.2 Tests: gate on all three mutation paths; disabled models exempt

## 4. Admin API and migration

- [x] 4.1 `/admin/api/models` CRUD + enable/disable + deployment management (admin-only), snapshot rebuild on commit
- [x] 4.2 Provider delete protection extended to deployments (409 naming model)
- [x] 4.3 One-time boot migration: routes+aliases → models/deployments (route order = priority), unpriced → disabled, alias-less chain providers warned; keep provider alias column data intact this release for rollback; dead `routes` warning
- [x] 4.4 Key API accepts allowed_models on create and edit (owner or admin)
- [x] 4.5 Integration tests: migration fixtures (aliased, alias-less, mixed), end-to-end model call after migration

## 5. Console UI

- [x] 5.1 Model catalog view: list/create/edit/delete, deployment editor with provider select + upstream name + reorder, enable/disable with 409 surfacing
- [x] 5.2 Key creation/edit: allowed-models multi-select from enabled catalog (empty = all)
- [x] 5.3 Remove config editor view and its routes; navigation gains Models entry
- [x] 5.4 Dashboard filters: model filter driven by catalog names

## 6. Documentation and verification

- [x] 6.1 README: model catalog concept, fixed endpoint paths, /v1/models behavior, migration notes (pass-through gaps, rollback window), client-facing breaking changes
- [x] 6.2 Full suite + manual verify: migrated config serves same traffic, catalog edit reroutes without restart, restricted key 403s
