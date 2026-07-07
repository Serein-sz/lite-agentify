## Why

The gateway now records per-request token usage and cost into PostgreSQL, but there is no way to read any of it back — the data is write-only. Likewise, the only way to inspect or change gateway configuration is to hand-edit the TOML file on the host. A built-in web admin console — served by the same single binary — makes usage visible and configuration manageable from a browser, with zero extra deployment moving parts.

## What Changes

- New **web admin console** under the reserved `/admin` path prefix on the existing listen port: a React SPA (login, usage dashboard, config editor) embedded into the binary.
- New optional **`admin_password`** top-level config field. On startup, a plaintext value is hashed with argon2id and **written back** to the config file (comments/formatting preserved). The admin console is enabled only when `admin_password` is configured; without it the gateway behaves as today.
- **Session auth** for the console: `POST /admin/api/login` verifies the password and issues an HttpOnly `SameSite=Strict` session cookie (in-memory session store). Failed logins are rate-limited. All `/admin/api/*` endpoints except login require a valid session.
- **Config management API**: `GET /admin/api/config` returns the raw TOML with secret values masked; `PUT /admin/api/config` validates the submitted TOML (same validation path as hot reload), restores masked secrets, writes the file atomically, and triggers an immediate reload. A content-hash guard rejects writes based on a stale copy (409).
- **Usage query API**: paginated request log (`GET /admin/api/usage`) and aggregated summary (`GET /admin/api/usage/summary` — totals, time series, per-provider/model breakdown) over the existing usage table.
- **Path-space change**: `/admin` and its subpaths are reserved by the gateway and never proxied upstream (alongside the existing `/healthz` and `/reload`).
- New frontend workspace `ui/` managed with **pnpm** (Vite + React + TypeScript); production build is embedded via rust-embed.

Not breaking: existing configs parse unchanged; all new behavior is opt-in via `admin_password`.

## Capabilities

### New Capabilities
- `admin-auth`: Admin password lifecycle (first-boot hash write-back), login/logout sessions, session gating of admin endpoints, login rate limiting.
- `admin-config`: Reading gateway config with masked secrets and writing it back with validation, secret round-trip, atomic persistence, concurrency guard, and immediate reload.
- `admin-usage`: Querying recorded usage — paginated request log and aggregated summaries for the dashboard.
- `admin-ui`: The embedded single-page admin console served under `/admin` (login, dashboard, config editor).

### Modified Capabilities
- `llm-gateway`: The `/admin` path prefix becomes reserved gateway path space — requests under it are handled by the admin console (or 404 when disabled) and are never forwarded to an upstream provider.

## Impact

- **Affected code**: new `src/admin/` module (router, auth/session, config API, usage queries, embedded assets); `src/config.rs` (`admin_password` field); `src/main.rs` (hash write-back before the config watcher starts); `src/proxy/router.rs` (mount admin router ahead of the proxy fallback). During implementation the `src/gateway/` wrapper was dissolved into a plane-based layout (`src/proxy/` data plane, `src/admin/` control plane, shared modules at the crate root).
- **New Rust dependencies**: `argon2` (password hashing), `toml_edit` (format-preserving config rewrite), `rust-embed` (asset embedding), `sha2` (config content hash).
- **New frontend workspace**: `ui/` — pnpm, Vite, React, TypeScript, Tailwind + shadcn/ui, ECharts, TanStack Query/Table, CodeMirror (TOML). Release builds run `pnpm build` before `cargo build` so `ui/dist` exists for embedding.
- **Database**: read-only queries against the existing usage table; no schema changes.
- **Security surface**: admin endpoints share the data-plane port and are gated by password sessions; config read masks secrets, config write implies custody of provider API keys (documented threat: rewriting `base_url` exfiltrates keys — hence auth + rate limiting are spec-level requirements).
- **Out of scope**: structured form-based config editing (v1 is a raw TOML editor), multiple admin users/roles, TLS termination, a separate admin listener (the admin router stays self-contained so one can be added later), usage retention/pruning.
