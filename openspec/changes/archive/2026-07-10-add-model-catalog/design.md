# Design: add-model-catalog

## Context

After changes 1–2, users/keys/providers/pricing live in Postgres and the snapshot rebuilds on mutation; routes remain in the file as path-prefix → provider chains, and per-provider `model_aliases` translate model names. The exploration settled the target: models are the only external surface; a model owns an ordered deployment chain (provider + upstream name) curated by admins; users and keys never see providers; no protocol translation (decision A); pricing coverage is a listing precondition.

## Goals / Non-Goals

**Goals:**
- Catalog-based resolution: body `model` → deployment chain, endpoint path → protocol filter.
- Same failover/retry/streaming-safety semantics as today over the filtered chain.
- Pricing-coverage gate on enabling a model.
- Key-level `allowed_models`; gateway-owned `/v1/models`.
- One-time migration from routes/aliases; delete the route machinery.

**Non-Goals:**
- Protocol translation (OpenAI⇄Anthropic) — future change if ever.
- Quotas/balances (change 4).
- Per-user chain overrides — explicitly rejected in exploration; the chain is a model attribute.
- Health/latency-aware dynamic reordering of deployments.

## Decisions

### D1: Resolution pipeline replaces route matching

`match_route(path, model)` becomes `resolve(protocol_of(path), model, key)`:

1. Look up `model` in the snapshot catalog → not found or disabled → protocol-native error (404-shaped, naming the model).
2. Check key's `allowed_models` → miss → 403 before any upstream contact.
3. Filter the model's ordered deployments to `deployment.provider.protocol == endpoint protocol` → empty → protocol-native error ("model not available on this endpoint").
4. Walk the filtered chain with today's failover + rate-limit retry, rewriting the top-level `model` to each deployment's `upstream_model` per attempt.

Path prefixes are fixed per protocol (`/v1/chat/completions`, `/v1/responses` → OpenAI; `/v1/messages` → Anthropic) instead of configurable route prefixes. Endpoints without a body `model` are no longer proxyable except gateway-owned ones (`/v1/models`, `/healthz`, `/reload`, `/admin`).

*Alternative*: keep configurable path prefixes alongside the catalog — rejected: two routing systems to explain and misconfigure; the fixed protocol paths are what clients use anyway.

### D2: Catalog shape — deployments as an ordered child table

`models(name PK, status, created_at)`; `model_deployments(id, model_name FK, provider_id FK, upstream_model, priority)` with `unique(model_name, provider_id)` and priority giving the failover order. Mixed-protocol deployments under one model are **allowed and useful** (the same model reachable from both endpoint families); homogeneity is enforced per-request by the protocol filter, not at config time. Config-time validation instead checks: deployments reference existing providers, non-empty upstream names, at least one deployment to enable.

*Alternative*: JSON deployments column on `models` — rejected: FK integrity to providers (change 2's delete-protection extends naturally: deleting a provider referenced by a deployment → 409).

### D3: Pricing gate at the state transitions that could break it

Invariant: an **enabled** model's every deployment resolves a pricing rule (wildcard fallback included). Enforced at: enabling a model, adding/editing a deployment of an enabled model, and deleting/editing a pricing rule that would strip coverage (409 naming the model). Disabled models are exempt (drafts). This is validation at mutation time, not request time — the hot path never re-checks.

*Alternative*: request-time check with "unpriced = block" — rejected: turns a config mistake into scattered runtime failures instead of one clear admin-time error.

### D4: `/v1/models` serves the catalog, scoped to the key

The gateway answers `GET /v1/models` itself: enabled models the presented key may use, rendered in the protocol-native list shape for the endpoint family. This stops leaking upstream provider catalogs (which listed models the gateway would refuse anyway) and gives clients an accurate discovery surface.

### D5: Migration converts what is derivable, reports the rest

One-time at boot (models table empty + file has routes): for each route, for each provider in its chain, for each alias `(public → upstream)` on that provider — upsert model `public` with deployment `(provider, upstream)` at the route's chain position. Providers in chains without aliases imply pass-through models that cannot be enumerated; log each such provider with a "create catalog entries manually" warning. Migrated models start **disabled** when pricing coverage is missing, enabled otherwise — the gate stays honest. `model_aliases` on providers are dropped by migration (column retired); `routes` becomes a dead section with the same warning treatment as change 2.

### D6: Key `allowed_models` is a nullable list, checked in the snapshot

`api_keys.allowed_models` (JSONB, NULL = all models). The snapshot's key entry carries the set; the check is an in-memory set lookup. Referential looseness is deliberate: entries naming since-deleted models are inert; the console's picker only offers cataloged models.

## Risks / Trade-offs

- [Clients relying on unknown-model pass-through break] → Intentional (the catalog IS the contract); clear protocol-native error naming the model; README migration notes.
- [Alias-less migration leaves gaps] → Loud per-provider warnings + catalog view shows an empty-catalog call-to-action; admin creates entries in minutes via UI.
- [Fixed protocol paths drop exotic path routing (e.g. custom prefixes)] → No known user; the config editor never supported creating new protocol shapes anyway.
- [Mixed-protocol model + endpoint filter surprises ("model exists but 404s here")] → Error message names the endpoint family and the families the model supports.
- [Catalog grows the snapshot rebuild scope] → Same human-scale mutation frequency; rebuild already reads providers/pricing.

## Migration Plan

1. Deploy; boot migration converts routes/aliases → catalog (D5), warns about gaps.
2. Admin reviews catalog view, creates missing pass-through models, enables everything priced.
3. Clients unchanged if they already sent public model names; clients using upstream-only names switch to catalog names.
4. Remove `routes` from the TOML at leisure.
5. Rollback: previous binary reads the file `routes` (untouched) and provider aliases — but aliases were dropped from provider rows by D5's column retirement; therefore the migration MUST NOT physically drop the column until the following release. Rollback within one release keeps working.

## Open Questions

- None blocking. Latency/health-aware deployment ordering and protocol translation are future candidates.
