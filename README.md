# lite-agentify

## Admin Console

A browser admin console lives under the reserved `/admin` prefix on the gateway's listen port: a usage dashboard (requests, tokens, cost, latency, error rate) and a config editor with save-and-hot-reload. `/admin` is gateway-owned path space — requests under it are never proxied upstream.

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

- Hot-reloadable fields: `providers` (including `model_aliases`), `routes`, `pricing`, `gateway_keys`, `admin_password`.
- Not hot-reloadable: `listen_addr` and `usage_database` — changes are ignored with a warning log and require a restart; the remaining fields still take effect.
- If the new config fails to parse or validate, the previous config keeps serving and the error is logged; the swap is atomic, so requests never see a partially applied config.
- In-flight requests finish with the config snapshot they started with.
- Gateway key changes apply immediately. To rotate keys without downtime: add the new key → reload → switch clients to it → remove the old key → reload.

## Usage Recording

Token usage and cost persistence is optional. If `usage_database` is absent or disabled, the gateway continues proxying requests without writing usage records. Pricing is deployment-managed configuration; the gateway does not fetch provider prices or hard-code model rates.

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
```
