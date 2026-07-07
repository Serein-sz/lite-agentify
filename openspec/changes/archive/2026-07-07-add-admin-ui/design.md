## Context

The gateway is a single-binary axum service. Its router owns three explicit paths (`/healthz`, `POST /reload`) and hands **everything else** to the proxy fallback (`router.rs`), so any admin surface must claim an explicit path prefix — it cannot use the fallback like a typical SPA host would.

Relevant current-state facts:

- Config lives in one TOML file (`config.rs`); `SharedGatewayState` (arc-swap) already supports hot reload, triggered by a directory watcher with a 500 ms debounce that survives editor rename-saves (`reload.rs`), or by `POST /reload`. Reload validation path: `GatewayState::from_config_with_upstream_and_recorder(...)` reusing the current upstream client and usage recorder.
- `listen_addr` and `usage_database` changes are warn-only on reload (restart required).
- Usage records are written to PostgreSQL via sea-orm (`usage/`), but nothing reads them back.
- Secrets present in config: `providers[].api_key`, `gateway_keys[]`, and the `usage_database.url` (may embed a DB password).
- Data-plane auth is bearer `gateway_keys`; there is no human-oriented auth.

Threat highlight that shaped this design: **config write access is equivalent to provider-key custody** — an attacker who can edit config can point a provider's `base_url` at their own server and receive the real API key in the next proxied request's headers. Admin auth is therefore a security boundary, not a UX nicety.

## Goals / Non-Goals

**Goals:**

- A browser admin console (usage dashboard + config editor) served by the same binary on the same port, enabled only when an admin password is configured.
- Password-gated sessions whose security holds on a network-exposed port (slow hashing, login lockout, hardened cookies).
- Config editing with the same safety net as hot reload: validate first, write atomically, reload immediately, never clobber concurrent manual edits silently.
- Single-file deployment: frontend assets embedded in the release binary.

**Non-Goals:**

- Structured (form-based) config editing — v1 exchanges raw TOML text.
- Multiple admin users, roles, or password change via the UI.
- TLS termination, CSRF tokens (covered by `SameSite=Strict`), or a separate admin listener (the admin router stays self-contained so a dedicated listener can be added later with a config flag).
- Usage data retention, pruning, or export.
- SSR — the SPA is fully static.

## Decisions

### Decision 1: Same port, reserved `/admin` prefix, self-contained admin Router

The admin surface mounts as `Router::nest_service("/admin", admin_router(...))` in `build_router_with_shared`, ahead of the proxy fallback (nested routes always win over the fallback). `/admin` joins `/healthz` and `/reload` as reserved gateway path space. When `admin_password` is absent, a stub router still claims the prefix and returns 404 — path semantics don't depend on configuration.

**Why `/admin` and not `/api` or `/ui`:** Ollama's native surface lives under `/api/*`, so a gateway fronting Ollama-style providers would collide. No known LLM provider uses `/admin`.

**Alternative considered — separate admin listener (default `127.0.0.1`):** stronger isolation, but a second port to operate and out of proportion for a lite tool whose data-plane port is already bearer-gated. Rejected for v1; the self-contained `admin_router()` keeps that door open.

### Decision 1b: Plane-based source layout (user decision during implementation)

The former `src/gateway/` wrapper module — which contained the entire crate — was dissolved. Modules now sit at the crate root grouped by plane: `src/proxy/` (data plane: router, upstream, headers), `src/admin/` (control plane), with shared infrastructure at the top level (`config.rs`, `state.rs`, `reload.rs`, `model.rs`, `domain/`, `pricing/`, `usage/`). `model.rs` stays top-level rather than inside `proxy/` because `Protocol` is shared by config, usage, and pricing — the control plane must not depend on the data plane for domain types.

### Decision 2: `admin_password` with first-boot hash write-back

New optional top-level config field `admin_password`. Boot sequence in `main.rs`:

```
load config ──▶ admin_password plaintext? ──▶ argon2id hash ──▶ toml_edit write-back
                    (no $argon2id$ prefix)                        (comments preserved)
                                   │
                                   ▼
                build router ──▶ spawn config watcher ──▶ serve
```

- Plaintext detection: a value not starting with `$argon2id$` is plaintext. Hashes are stored in PHC string format, which argon2's verifier consumes directly.
- Write-back uses `toml_edit` (format/comment-preserving — the same reason cargo uses it), and runs **before** the watcher spawns, so it cannot trigger a spurious reload.
- If write-back fails (e.g. read-only file), log a prominent warning and continue with the in-memory hash: the gateway stays usable, but the plaintext remains on disk until fixed.

**Alternative considered — separate `admin_password_hash` field with a CLI hash command:** no self-mutating config, but worse UX (extra manual step) and the user explicitly chose first-boot hashing.

### Decision 3: In-memory sessions outside the swappable state

`AdminState { shared: SharedGatewayState, sessions, login_limiter, config_path }` is the admin router's state. Sessions live **beside** — not inside — the arc-swapped `GatewayState`, so a config reload never logs anyone out.

- Token: ≥128-bit OS-random value; cookie `lite_agentify_admin`, `HttpOnly`, `SameSite=Strict`, `Path=/admin`, 24 h absolute TTL, lazily expired. No `Secure` flag because the gateway itself may legitimately serve plain HTTP on localhost.
- `SameSite=Strict` + custom-path cookie is the CSRF story: cross-site requests never carry the cookie.
- Restart logs all admins out — accepted for a single-admin tool.
- Gating: an axum middleware on the admin router requires a valid session for every `/admin/api/*` route except `POST /admin/api/login`; static assets (login shell) are public.

### Decision 4: Global login lockout, not per-IP

After 5 consecutive failed logins, all logins are rejected for 60 s (in-memory counter, reset on success or window expiry). Combined with argon2id's ~100 ms verify cost, network brute force is impractical.

**Alternative considered — per-IP limiting:** finer-grained, but client IP is unreliable behind proxies (`X-Forwarded-For` is spoofable) and a global lockout is strictly safer for a single-admin console.

### Decision 5: Config exchange format is raw TOML text with sentinel-masked secrets

`GET /admin/api/config` → `{ content, hash }` where `content` is the on-disk TOML with every secret value replaced by `__MASKED__<last4>` (via toml_edit, preserving layout), and `hash` is SHA-256 of the raw file bytes. Masked fields: `providers[].api_key`, `gateway_keys[]`, `usage_database.url`, `admin_password`.

`PUT /admin/api/config` with `{ content, base_hash }`:

1. **Concurrency guard**: if SHA-256 of current file ≠ `base_hash` → `409` with the fresh content+hash (someone edited the file meanwhile; UI re-loads and the user re-applies).
2. **Unmask**: every value starting with `__MASKED__` is restored from the current on-disk file — `api_key` matched by provider `id`, `usage_database.url` and `admin_password` positionally, `gateway_keys` by index only when the list length is unchanged. An unresolvable sentinel (new provider id with a masked key; changed-length `gateway_keys` containing sentinels) → `400` with a message naming the field.
3. **Validate before write**: parse into `GatewayConfig`, then construct a `GatewayState` exactly like reload does. Any failure → `400` + error, file untouched.
4. **Atomic persist**: write temp file in the config directory, `fs::rename` over the original (std rename replaces existing files on Windows; the watcher already tolerates rename-based replacement).
5. **Immediate reload**: call `reload::reload(&shared)` synchronously so the response can report success; the watcher's debounced duplicate reload is idempotent. Restart-required diffs (`listen_addr`, `usage_database`) are returned as `warnings[]` in the response.

**Alternative considered — structured JSON config API:** enables form UIs but requires lossy TOML↔JSON round-trips (comments die) and a much larger API surface. Raw text + `toml_edit` keeps user comments sacred; forms can layer on later.

### Decision 6: Usage read API is two endpoints over the existing table

- `GET /admin/api/usage?from&to&provider&model&status&page&page_size` — paginated log, `created_at` desc, `page_size` capped at 200. `status` filters by exact code or class (`4xx`/`5xx`).
- `GET /admin/api/usage/summary?from&to&bucket=hour|day` — one response for the whole dashboard: totals (request count, tokens, cost grouped by currency, avg latency, error rate), a time series bucketed via `date_trunc`, and per-provider×model breakdown.
- When `usage_database` is disabled, both return `200` with `"usage_enabled": false` and empty data — the UI renders an explicit empty state instead of an error.

Aggregation runs as SQL (sea-orm queries); no pre-aggregation tables at this data volume.

### Decision 7: Frontend — pnpm workspace at `ui/`, Vite + React SPA

- **Toolchain**: pnpm (project convention), Vite, React, TypeScript.
- **UI**: Tailwind CSS with hand-rolled components for v1 (implementation deviation: shadcn/ui was skipped — its CLI/registry setup outweighed three pages of simple components; the `@/` path alias and Tailwind v4 are in place so shadcn components can be dropped in later). ECharts for the time-series/breakdown charts (thin custom React wrapper); TanStack Query for server state; TanStack Table for the request log; CodeMirror 6 with TOML highlighting for the config editor.
- **Routing**: react-router v7 with `basename: '/admin'`; Vite `base: '/admin/'`.
- **Dev loop**: `pnpm dev` serves on :5173 and proxies `/admin/api` to the running gateway — frontend iteration never touches the Rust build. No CORS in either mode (dev = proxy, prod = same origin).

### Decision 8: Assets embedded via rust-embed

`#[derive(RustEmbed)] #[folder = "ui/dist"]` serving handler: `/admin` and any non-asset subpath → `index.html` (`Cache-Control: no-cache`) so client-side routes deep-link correctly; hashed build assets → `Cache-Control: immutable` + MIME from extension. rust-embed reads from disk in debug builds and embeds at compile time in release — `ui/dist/.gitkeep` is committed so `cargo build` works without a frontend build; release packaging runs `pnpm build` first.

**Alternative considered — tower-http `ServeDir`:** no compile-time coupling, but reintroduces a runtime directory dependency, breaking single-file deployment. Rejected as the primary path; not needed even for dev thanks to the Vite proxy.

## Risks / Trade-offs

- **Admin on the data-plane port** → brute-force exposure on whatever network the proxy serves. Mitigation: argon2id verify cost + global lockout (Decision 4) + hardened cookie; docs recommend firewalling/binding appropriately for internet-facing deployments.
- **Config write = provider-key custody** (base_url redirect exfiltration) → Mitigation: the above auth boundary; secrets masked on read so a read-only compromise (e.g. shoulder-surf, screenshot) leaks nothing; sentinel round-trip never echoes secrets back to the browser.
- **Sentinel ambiguity** (masked `gateway_keys` in a re-ordered/resized list; masked key under a renamed provider) → Mitigation: hard `400` naming the field rather than guessing; user re-enters the real value.
- **A real secret could theoretically start with `__MASKED__`** → Mitigation: accepted; on PUT such a value resolves as a sentinel — pathological and self-inflicted.
- **Concurrent editor holding the file open on Windows can fail the rename** (sharing violation) → Mitigation: single retry after short delay; on persistent failure return 500 with the OS error, file unchanged.
- **In-memory sessions/lockout reset on restart** → accepted for a single-admin lite tool.
- **Binary size grows by the embedded SPA (~2–5 MB)** → accepted for single-file deployment.
- **Release builds require Node/pnpm** → Mitigation: `.gitkeep` keeps plain `cargo build` green (empty console); release/packaging docs make `pnpm build && cargo build --release` the canonical sequence.
- **usage summary over a large table could get slow** → Mitigation: time-range predicates on the indexed `created_at`; acceptable at lite scale; pre-aggregation stays out of scope.
