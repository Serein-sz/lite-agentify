# lite-agentify

## Admin Console

A browser admin console lives under the reserved `/admin` prefix on the gateway's listen port: a usage dashboard (requests, tokens, cost, latency, error rate) and a structured config editor with save-and-hot-reload. `/admin` is gateway-owned path space — requests under it are never proxied upstream.

### Config editor

The config page edits the hot-reloadable configuration as a structured form — `gateway_keys`, `providers` (including `model_aliases`), `routes`, and `pricing` — instead of raw TOML. Reference fields are guided: a route's providers are picked from the configured provider list (order = failover priority), and pricing rules choose a provider from that list or the `*` wildcard. Saves are reconciled into the on-disk TOML document, so comments and formatting on untouched entries survive. Restart-only settings (`listen_addr`, `usage_database`) and `admin_password` are not editable in the form — edit the file directly.

### Enabling

The console is disabled unless `admin_password` is set (everything under `/admin` returns 404). Add it as plaintext once:

```toml
admin_password = "choose-a-strong-password"
```

On the next startup the gateway replaces the value **in the config file** with its argon2id hash (comments and formatting are preserved). Logins verify against the hash; the plaintext is never stored again. If the file is read-only the gateway warns and continues with an in-memory hash — the plaintext stays on disk until the file becomes writable.

### Security notes

- Login sets an `HttpOnly`, `SameSite=Strict` session cookie scoped to `/admin` (24 h TTL, in-memory: restarting the gateway logs everyone out; hot reloads do not).
- After 5 consecutive failed logins, all logins are rejected for 60 s — combined with argon2's slow verification this makes network brute force impractical.
- The config editor implies custody of provider API keys (editing a `base_url` redirects traffic). Secrets are masked as `__MASKED__…` in the editor and round-trip unchanged; still, expose the port beyond localhost/LAN deliberately and firewall accordingly.
- Each secret field has a copy button that fetches that one secret's plaintext on demand (`POST /admin/api/config/reveal`, session-gated, single field per request) and writes it to the clipboard. This is the only path where a plaintext secret reaches the browser — `GET /admin/api/config` always stays fully masked.
- Config saves are validated first (invalid config → 400, file untouched), written atomically, hot-reloaded immediately, and rejected with 409 if the file changed on disk since it was loaded.

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
- **Endpoint**: `POST /reload` with a gateway key, e.g. `curl -X POST -H "Authorization: Bearer <gateway-key>" http://<listen_addr>/reload`. Returns 200 on success, or 500 with the failure reason.

Behavior:

- Hot-reloadable fields: `providers` (including `model_aliases`), `routes`, `pricing`, `gateway_keys`, `admin_password`, `retry`.
- Not hot-reloadable: `listen_addr` and `usage_database` — changes are ignored with a warning log and require a restart; the remaining fields still take effect.
- If the new config fails to parse or validate, the previous config keeps serving and the error is logged; the swap is atomic, so requests never see a partially applied config.
- In-flight requests finish with the config snapshot they started with.
- Gateway key changes apply immediately. To rotate keys without downtime: add the new key → reload → switch clients to it → remove the old key → reload.

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

Token usage and cost persistence is optional. If `usage_database` is absent or disabled, the gateway continues proxying requests without writing usage records. Pricing is deployment-managed configuration; the gateway does not fetch provider prices or hard-code model rates.

Usage records are written asynchronously: the proxy hands each record to a background writer that batches inserts, so the response path never waits on the database. This means the dashboard is eventually consistent — a just-completed request may not appear for up to the flush interval (~1s). On graceful shutdown (Ctrl-C / SIGTERM) the gateway drains and flushes the pending batch before exiting; a hard kill (SIGKILL) can still drop the in-memory buffer, which is acceptable because usage recording is best-effort and never blocks or fails a client response.

Example TOML:

```toml
# [usage_database]
# enabled = true
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

Create the PostgreSQL table before enabling persistence:

```sql
CREATE TABLE IF NOT EXISTS usage_records (
    id uuid PRIMARY KEY,
    request_id text NOT NULL,
    created_at timestamptz NOT NULL,
    provider_id text NOT NULL,
    protocol text NOT NULL,
    path text NOT NULL,
    requested_model text NULL,
    upstream_model text NULL,
    status integer NOT NULL,
    latency_ms bigint NOT NULL,
    input_tokens bigint NULL,
    output_tokens bigint NULL,
    cached_input_tokens bigint NULL,
    cache_read_tokens bigint NULL,
    cache_write_tokens bigint NULL,
    total_tokens bigint NULL,
    estimated_cost numeric(20, 10) NULL,
    currency text NULL,
    usage_source text NOT NULL,
    pricing_source text NULL
);

-- Indexes for the dashboard's time-range, provider, and model queries.
CREATE INDEX IF NOT EXISTS idx_usage_records_created_at
    ON usage_records (created_at DESC);
CREATE INDEX IF NOT EXISTS idx_usage_records_provider_created_at
    ON usage_records (provider_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_usage_records_upstream_model
    ON usage_records (upstream_model);
```

Usage records are written asynchronously by a background batch writer: the proxy response path never waits on the database. Records land within the flush interval (batched by count or ~1s), so the admin dashboard can lag the most recent request by up to that window. On graceful shutdown (Ctrl-C / SIGTERM) the gateway drains and flushes the pending batch before exiting; a hard kill (SIGKILL) can still drop the last unflushed records, which is acceptable because usage recording is best-effort and never blocks or fails a client response.
