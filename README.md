# lite-agentify

## Model catalog and routing

Clients call **models**, not providers. The catalog is the routing contract:

- A **model** is the public name a client sends (`"model": "gpt-chat"`). Each model owns an ordered **deployment chain**: `(provider, upstream model name)` pairs curated by admins. The gateway tries deployments in order with the same failover/rate-limit-retry behavior as before, rewriting the body's `model` to each deployment's upstream name per attempt.
- Endpoint paths are fixed per protocol: `POST /v1/chat/completions` and `POST /v1/responses` (OpenAI family), `POST /v1/messages` (Anthropic family). Other paths are no longer proxied. A model may have deployments on both protocol families; each request only uses deployments matching its endpoint's protocol.
- `GET /v1/models` is answered by the gateway itself: the enabled models the presented key may call, in the protocol-native shape (Anthropic's when an `anthropic-version` header is present, OpenAI's otherwise). Upstream provider catalogs are never exposed.
- A model can be listed (**enabled**) only when every deployment resolves a pricing rule (wildcards count). Enabling an unpriced model, editing an enabled model's chain into an unpriced state, or deleting/narrowing a pricing rule that an enabled model depends on is rejected with `409`. Disabled models are drafts and exempt.
- Requests resolve entirely in memory before any upstream contact: unknown/disabled model → protocol-native 404; model not allowed for the key → 403; no deployment on the endpoint's protocol → 404 naming the protocols that would work.

### Breaking changes vs. path-prefix routes

- Unknown models are no longer passed through to an upstream; the catalog is authoritative. Clients must send cataloged model names.
- Only the fixed protocol endpoints above are proxied (plus gateway-owned `/v1/models`, `/healthz`, `/reload`, `/admin`). Custom path prefixes and non-model endpoints (e.g. `/v1/embeddings`) no longer route.
- The request body must include a string `model` field.

### Migrating from `routes` + provider `model_aliases`

On the first boot with an empty `models` table and `[[routes]]` in the config file, the gateway derives the catalog: for each route, each provider's aliases `(public → upstream)` become deployments of model `public` at the route's chain position. Models with full pricing coverage start enabled; the rest start disabled (add pricing, then enable in the console). Providers in chains **without** aliases imply pass-through models that cannot be enumerated — each is logged with a reminder to create its catalog entries manually.

Afterwards the file's `routes` are dead (a startup warning flags them) — delete them at leisure. Provider alias data is kept in the database this release so a pre-catalog binary can still roll back; the following release retires it.

## Accounts and API keys

The gateway authenticates proxied requests with **database-backed API keys**, each owned by a user account. PostgreSQL is required (see [Database](#database)); accounts, keys, and usage records all live there.

### Roles

- **admin** — manages users (create, disable, reset password), sees all usage, and edits the remaining file config. The bootstrap `admin` user is seeded on first boot (below).
- **user** — self-service: creates and revokes their own API keys, changes their own password, and sees only their own usage.

### First boot and bootstrap admin

On the first startup against an empty `users` table, the gateway seeds an `admin` user from the `admin_password` config value. Set it as plaintext once:

```toml
admin_password = "choose-a-strong-password"
```

On startup the gateway replaces the value **in the config file** with its argon2id hash (comments and formatting preserved), then seeds the `admin` user from it. Once `users` is non-empty this value is ignored — change passwords through the console. If the file is read-only the gateway warns and continues with an in-memory hash.

### Migrating from `gateway_keys`

The old static `gateway_keys` list is no longer used for authentication. On the first boot with an empty `api_keys` table, any `gateway_keys` still in the config are imported once as active API keys owned by the bootstrap admin, so existing clients keep working. After verifying, create per-user keys through the console and delete `gateway_keys` from the file (a lingering field only produces a warning).

### API keys

A user creates a key in the console; the plaintext (`la-…`) is shown exactly once at creation and never again — only its SHA-256 hash and an `la-…` display prefix are stored. Clients present it as `Authorization: Bearer la-…` (or `x-api-key`/`api-key`). Revoking a key, or disabling its owner, stops it authenticating on the next snapshot refresh.

A key can optionally be restricted to specific models (`allowed_models`, editable by the owner or an admin). An unrestricted key may call every enabled model; a restricted key gets `403` for anything else, and its `GET /v1/models` listing only shows what it may call. A key can also carry a cumulative USD spend cap (`spend_cap_usd`, owner- or admin-editable): once the key's lifetime spend reaches the cap it gets `402` (see below), independent of the owner's remaining balance and of the owner's other keys.

## Credits and quotas (prepaid)

Spending is **prepaid and cumulative**: an admin grants credit (USD) to a user, and every request's estimated cost counts against it. There are no billing periods or resets — balance = Σ grants − Σ usage cost, always derived from the ledger, never stored.

- **Grants** are admin actions: the console's 额度 page or `POST /admin/api/credits/grants` (`{"user_id", "amount_usd", "note"}`). Negative amounts are corrections. The `credit_grants` ledger is append-only and records who granted what and why; users see their own balance on the dashboard (`GET /admin/api/me/balance`).
- **Enforcement is pre-flight and soft**: the gateway checks in-memory counters before any upstream contact (zero database access on the request path). A user whose cumulative spend has reached their granted total gets a protocol-native `402` with code `insufficient_quota`; a key that reached its `spend_cap_usd` gets the same naming the key cap. `429` stays reserved for upstream rate limits.
- **Soft-limit semantics**: costs are counted after responses complete, so concurrent in-flight requests can overshoot a balance by their own cost. Counters are reconciled against Postgres-recomputed truth every 60 s (healing memory-mode crash loss and Redis drift alike). This is deliberate: availability over exact cutoffs.
- **Rollout note**: users start with **zero balance** — their requests `402` until the first grant. Grant credit before announcing keys.

## Redis (optional hot-state backend)

Without Redis, everything below lives in process memory and works fine for a single instance. Add the optional `[redis]` section and restart to move the gateway's hot state into Redis:

```toml
[redis]
url = "redis://:STRONG-PASSWORD@127.0.0.1:6379/0"   # secret — treat like database.url
```

What moves there:

- **Spend counters** (`spent:user:{id}`, `spent:key:{id}`) — survive gateway restarts. During a Redis outage they degrade to an in-memory shadow (requests keep serving; one warning per outage window) and are re-seeded from Postgres truth by the reconciliation loop once Redis returns.
- **Admin sessions** (`session:{token}`, native 24 h TTL) — console logins survive gateway restarts. Session reads during an outage **fail closed**: the console answers 401 until Redis is back; auth never fails open.
- **Login lockout** (`lockout:{username}`, TTL = the lockout window).
- The reserved **`config_changed` pub/sub channel**: snapshot-affecting console mutations publish to it. In this single-instance release the subscriber is a deliberate no-op (the mutating instance already refreshed its own snapshot); the channel exists so multi-instance fan-out can be added without a protocol change.

The section is restart-only, like `[database]`. Postgres remains the source of truth for everything — Redis holds only derived or expiring state, so losing it costs at most active console sessions and one reconciliation interval of counter freshness.

**Security**: session tokens live in Redis, so treat Redis access as admin-console access. Set a strong `requirepass` (replace any weak or default password), bind Redis to a private interface, and firewall the port. The `url` carries the password, which makes the config file a secret store — keep it out of version control and readable only by the gateway's service account.

## Providers and pricing

Providers, pricing rules, and the model catalog are stored in PostgreSQL and managed through the admin console (or the `/admin/api/providers`, `/admin/api/pricing`, and `/admin/api/models` endpoints), not the config file. Changes take effect immediately via a snapshot rebuild — no restart, no file edit.

- **Providers**: id, protocol, base URL, upstream API key, optional Anthropic version. Upstream keys are masked in list/read responses and revealed one at a time via `POST /admin/api/providers/<id>/reveal`. Deleting a provider still used by a model deployment is rejected with `409` naming the model.
- **Pricing**: provider (or `*`), model (or `*`), per-million token rates, currency. The wildcard fallback order is unchanged (provider+model → provider+`*` → `*`+model → `*`+`*`). Deleting or narrowing a rule that an **enabled** model depends on is rejected with `409`.

### Migrating from file-based providers/pricing

On the first boot with empty `providers`/`pricing` tables, any `[[providers]]` and `[[pricing]]` sections in the config file are imported once into the database. Afterwards those file sections are ignored (a startup warning flags them) — delete them from the file. Provider upstream keys are stored **plaintext** in the database, protected by database access control; treat database access as custody of every upstream credential. Rolling back to a pre-change binary keeps working because the file sections are never modified by the import.

### Security notes

- Login takes a username and password; failures are identical for unknown user, disabled user, and wrong password, so login does not reveal which usernames exist.
- Login sets an `HttpOnly`, `SameSite=Strict` session cookie scoped to `/admin` (24 h TTL). Sessions live in memory by default — restarting the gateway logs everyone out (hot reloads do not) — or in Redis when `[redis]` is configured, where they survive restarts and fail closed during a Redis outage.
- After 5 consecutive failed logins **for a username**, logins for that username are rejected for 60 s — combined with argon2's slow verification this makes network brute force impractical, and one attacker cannot lock out other users.
- Managing providers implies custody of upstream API keys (editing a `base_url` redirects traffic). Provider keys are stored in the database, masked as `__MASKED__…` in API responses, and revealed one field at a time; expose the port beyond localhost/LAN deliberately and firewall accordingly.
- Catalog mutations are validated before the snapshot swap: a change that would break serving (e.g. a deployment referencing a deleted provider) commits to the database but the previous snapshot keeps serving and the response carries a warning.

## Database

PostgreSQL is a **hard dependency**: the gateway stores user accounts, API keys, and usage records there and refuses to start without a reachable database. Configure it in the `[database]` section (the old `[usage_database]` name is accepted as a deprecated alias):

```toml
[database]
url = "postgres://user:password@host:5432/dbname"
max_connections = 5
```

Schema migrations run automatically at startup — no manual table creation is needed.

### Building the console

The frontend is a pnpm + Vite + React SPA in `ui/`, embedded into release binaries by rust-embed:

```bash
cd ui && pnpm install && pnpm build   # produces ui/dist
cargo build --release                 # embeds ui/dist into the binary
```

Plain `cargo build` works without a frontend build (`ui/dist` ships only a `.gitkeep`; the console then serves a "assets not built" hint in debug). For frontend development run the gateway locally and `cd ui && pnpm dev` — Vite serves on :5173 and proxies `/admin/api` to the gateway, no Rust rebuild needed.

## Config Hot Reload

The gateway reloads its config file at runtime without a restart. Two triggers share the same reload logic:

- **File watching**: the gateway watches the config file (the one resolved at startup from `LITE_AGENTIFY_GATEWAY_CONFIG` or the default path) and reloads automatically after saves, debounced ~500ms.
- **Endpoint**: `POST /reload` with an API key, e.g. `curl -X POST -H "Authorization: Bearer <api-key>" http://<listen_addr>/reload`. Returns 200 on success, or 500 with the failure reason.

Behavior:

- Hot-reloadable file fields: `retry` (the only remaining hot-reloadable file section).
- Not hot-reloadable: `listen_addr` and `database` — changes are ignored with a warning log and require a restart; the remaining fields still take effect.
- Providers, pricing, the model catalog, accounts, and API keys live in the database, not the file: they take effect through a snapshot refresh triggered by their management APIs, without a file reload.
- If the new config fails to parse or validate, the previous config keeps serving and the error is logged; the swap is atomic, so requests never see a partially applied config.
- In-flight requests finish with the config snapshot they started with.

## Rate-Limit Retry

When an upstream provider returns a rate-limit status, the gateway waits and retries the **same** provider a few times before advancing the failover chain. This targets the most common recoverable upstream error: a transient 429/529 that a short backoff usually clears, where switching providers immediately would waste the primary and hammer a fallback that may also be limited.

The `[retry]` section is optional and hot-reloadable; an absent section uses the defaults shown:

```toml
[retry]
# Upstream statuses that trigger a backed-off retry against the same provider.
retryable_statuses = [429, 529]
# Total attempts per provider, including the initial try (must be >= 1).
max_attempts = 4
# First backoff wait; subsequent waits grow toward max_delay_ms.
base_delay_ms = 1000
# Upper bound on any single wait, also capping a large Retry-After.
max_delay_ms = 8000
```

Behavior:

- On a retryable status the gateway waits, then retries the same provider, up to `max_attempts` total. Only when attempts are exhausted does it advance to the next provider in the chain.
- The wait honors a `Retry-After` response header (seconds or HTTP-date form) when present, capped at `max_delay_ms`; otherwise it uses exponential backoff (`base_delay_ms` doubling toward `max_delay_ms`) with full jitter to avoid a thundering herd against a limited provider.
- Transport errors and HTTP 5xx still fail over **immediately** with no same-provider retry (unchanged). Other 2xx/3xx/4xx responses are forwarded to the client as-is.
- If the chain's last provider is still rate-limited after its retries, the gateway forwards that real rate-limit response (including any `Retry-After`) to the client rather than a synthetic 502.

## Usage Recording

Usage and cost are recorded in the required PostgreSQL database (see [Database](#database)). Each record is attributed to the user and API key that made the request; users see only their own usage in the console, admins see everything. Pricing is deployment-managed configuration; the gateway does not fetch provider prices or hard-code model rates.

Usage records are written asynchronously: the proxy hands each record to a background writer that batches inserts, so the response path never waits on the database. This means the dashboard is eventually consistent — a just-completed request may not appear for up to the flush interval (~1s). On graceful shutdown (Ctrl-C / SIGTERM) the gateway drains and flushes the pending batch before exiting; a hard kill (SIGKILL) can still drop the in-memory buffer, which is acceptable because usage recording is best-effort and never blocks or fails a client response.

Example pricing TOML:

```toml
# [database]
# url = "postgres://user:password@host:5432/dbname"
# url = "postgres://lite_agentify:password@localhost:5432/lite_agentify"
# max_connections = 5

# [[pricing]]
# provider = "openai"
# model = "gpt-4.1"
# input_per_1m = "2.00"
# output_per_1m = "8.00"
# cached_input_per_1m = "0.50"
# currency = "USD"
# pricing_source = "manual-2026-07"

# [[pricing]]
# provider = "anthropic"
# model = "claude-sonnet-4"
# input_per_1m = "3.00"
# output_per_1m = "15.00"
# cache_read_per_1m = "0.30"
# cache_write_per_1m = "3.75"
# currency = "USD"
# pricing_source = "manual-2026-07"

# Fallback pricing is supported with explicit "*" wildcards.
# Lookup order is:
# 1. provider + model
# 2. provider + "*"
# 3. "*" + model
# 4. "*" + "*"
#
# [[pricing]]
# provider = "*"
# model = "gpt-4.1"
# input_per_1m = "2.00"
# output_per_1m = "8.00"
# cached_input_per_1m = "0.50"
# currency = "USD"
# pricing_source = "global-gpt-4.1"
#
# [[pricing]]
# provider = "*"
# model = "*"
# input_per_1m = "1.00"
# output_per_1m = "3.00"
# currency = "USD"
# pricing_source = "global-default"
```

Schema (the `usage_records` table, the `users`/`api_keys` account tables, and their indexes) is created automatically by migrations at startup — no manual DDL is required.

Usage records are written asynchronously by a background batch writer: the proxy response path never waits on the database. Records land within the flush interval (batched by count or ~1s), so the admin dashboard can lag the most recent request by up to that window. On graceful shutdown (Ctrl-C / SIGTERM) the gateway drains and flushes the pending batch before exiting; a hard kill (SIGKILL) can still drop the last unflushed records, which is acceptable because usage recording is best-effort and never blocks or fails a client response.
