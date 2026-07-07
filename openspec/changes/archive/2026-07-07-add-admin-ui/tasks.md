## 1. Dependencies and config field

- [x] 1.1 Add `argon2`, `toml_edit`, `rust-embed`, and `sha2` to `Cargo.toml`
- [x] 1.2 Add optional `admin_password: Option<String>` to `GatewayConfig` in `src/config.rs`

## 2. Password bootstrap (hash write-back)

- [x] 2.1 Create `src/admin/password.rs`: detect plaintext vs `$argon2id$` PHC string, hash with argon2id, verify helper
- [x] 2.2 Implement config write-back via `toml_edit` (replace only the `admin_password` value, preserve comments/formatting)
- [x] 2.3 Wire into `src/main.rs` startup: run detection + write-back after config load and **before** `spawn_config_watcher`; on write failure warn and continue with in-memory hash
- [x] 2.4 Tests: plaintext gets hashed and written back with comments preserved; hashed value leaves file untouched; write failure still boots

## 3. Admin state, sessions, and login

- [x] 3.1 Create `src/admin/mod.rs` with `AdminState { shared, sessions, limiter, session_ttl }` — sessions live outside the arc-swapped `GatewayState`
- [x] 3.2 Implement in-memory session store: ≥128-bit OS-random tokens, 24 h TTL, lazy expiry
- [x] 3.3 Implement `POST /admin/api/login` (argon2 verify → set `HttpOnly`, `SameSite=Strict`, `Path=/admin` cookie) and `POST /admin/api/logout`
- [x] 3.4 Implement global login lockout: 5 consecutive failures → reject with 429 for 60 s, reset on success or window expiry
- [x] 3.5 Implement session-gating middleware for all `/admin/api/*` routes except login
- [x] 3.6 Tests: login success/failure, cookie attributes, gating 401s, expired session, lockout including correct-password-during-lockout, logout invalidation, session survives `reload()`

## 4. Config management API

- [x] 4.1 Create `src/admin/config_api.rs`: mask secrets (`providers[].api_key`, `gateway_keys[]`, `usage_database.url`, `admin_password`) in TOML text via `toml_edit` with `__MASKED__<last4>` sentinels
- [x] 4.2 Implement `GET /admin/api/config` returning `{ content, hash }` (SHA-256 of raw file bytes)
- [x] 4.3 Implement sentinel unmasking on submit: `api_key` by provider id, `usage_database.url`/`admin_password` positionally, `gateway_keys` by index only when length unchanged; unresolvable sentinel → 400 naming the field
- [x] 4.4 Implement `PUT /admin/api/config`: base-hash guard (mismatch → 409 with fresh content), validate via `GatewayState::from_config_with_upstream_and_recorder`, atomic write (temp file + rename, one retry on Windows sharing violation), then synchronous `reload()`; include restart-required warnings (`listen_addr`, `usage_database`) in the response
- [x] 4.5 Tests: masking hides every secret; round-trip preserves untouched secrets and persists replaced ones; invalid TOML / invalid config → 400 and file unchanged; stale hash → 409 and file unchanged; valid write activates new routes without restart; restart-required warning present

## 5. Usage query API

- [x] 5.1 Create `src/usage/query.rs` (trait + types) and `src/admin/usage_api.rs`: `GET /admin/api/usage` — sea-orm query with `from`/`to`/`provider`/`model`/`status` (exact or `4xx`/`5xx`) filters, `created_at` desc, pagination capped at 200, total count
- [x] 5.2 Implement `GET /admin/api/usage/summary`: totals (requests, tokens, cost by currency, avg latency, error rate), `date_trunc` time series (`hour`/`day`), provider×model breakdown
- [x] 5.3 Both endpoints return `200` + `usage_enabled: false` + empty data when no usage database is configured
- [x] 5.4 Tests: pagination/filter behavior, summary shape, disabled-database envelope (via `MemoryUsageRecorder` implementing the query trait)

## 6. Admin router assembly and embedded assets

- [x] 6.1 Create `src/admin/assets.rs`: `RustEmbed` over `ui/dist`; handler serving `index.html` (no-cache) for `/admin` and non-asset subpaths, hashed assets with correct MIME + immutable cache
- [x] 6.2 Commit `ui/dist/.gitkeep` so `cargo build` succeeds without a frontend build
- [x] 6.3 Build `admin_router()` (login/logout/config/usage/assets + middleware) and nest at `/admin` in `build_router_with_shared` in `src/proxy/router.rs`; when `admin_password` is absent nest a stub that always 404s
- [x] 6.4 Tests: `/admin/*` never reaches the proxy fallback (enabled and disabled); non-admin paths still proxy; SPA deep-link path returns index.html; asset returns correct MIME (fixture-embed unit tests)

## 7. Frontend scaffold (`ui/`, pnpm)

- [x] 7.1 Scaffold `ui/` with pnpm + Vite + React + TypeScript; set Vite `base: '/admin/'` and dev proxy `/admin/api` → `http://127.0.0.1:8080`
- [x] 7.2 Add Tailwind CSS + shadcn/ui, TanStack Query, react-router with `basename: '/admin'`
- [x] 7.3 Build the API client (fetch wrapper: JSON, credentials, 401 → redirect to login) and QueryClient setup
- [x] 7.4 Build the login page and auth flow: login form → `POST /admin/api/login`, global 401 handling returns to login, logout button
- [ ] 7.5 Verify `pnpm dev` end-to-end against a locally running gateway (login round trip through the proxy)

## 8. Dashboard page

- [x] 8.1 Time-range picker (e.g. today / 7d / 30d / custom) driving all dashboard queries
- [x] 8.2 Summary cards from `/admin/api/usage/summary`: requests, tokens, cost per currency, avg latency, error rate
- [x] 8.3 ECharts time-series chart (cost + tokens per bucket) and provider×model breakdown view
- [x] 8.4 Request log table (TanStack Table): server-side pagination and filters over `/admin/api/usage`
- [x] 8.5 Explicit "usage recording disabled" empty state when `usage_enabled: false`

## 9. Config editor page

- [x] 9.1 CodeMirror 6 editor with TOML syntax highlighting, loading masked content + hash from `GET /admin/api/config`
- [x] 9.2 Save flow: `PUT` with base hash; success toast including restart-required warnings; 400 shows the validation error and keeps the draft
- [x] 9.3 409 conflict flow: notify, offer "load current version", never silently discard edits

## 10. Integration, packaging, verification

- [ ] 10.1 Full round trip: `pnpm build` → `cargo build --release` → run binary from an empty directory → login, dashboard, config edit + hot reload all work from the single file
- [x] 10.2 Document the admin console in `README`/config docs: `admin_password` lifecycle, build order for release, security notes (exposure, lockout)
- [x] 10.3 Run `cargo build && cargo test` and `pnpm build` (plus lint), fixing any failures
