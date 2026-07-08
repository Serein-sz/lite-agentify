## 1. Structured config data model

- [x] 1.1 Define a serde `Serialize`/`Deserialize` structured config DTO in `src/admin/config_api.rs` covering the editable fields (`gateway_keys`, `providers` incl. `model_aliases`, `routes`, `pricing`) with secret fields as plain strings that may carry the `__MASKED__` sentinel
- [x] 1.2 Add a request type for `PUT /admin/api/config/structured` (structured config DTO + `base_hash`) and for `POST /admin/api/config/reveal` (single field reference)

## 2. Structured write endpoint (reconcile into toml_edit)

- [x] 2.1 Implement reconcile: load current on-disk TOML as `toml_edit::DocumentMut`, write submitted scalar values in place, add table entries for new array items, remove tables for deleted ones (comments on surviving nodes preserved)
- [x] 2.2 Reuse the existing masked-sentinel resolution so untouched secrets round-trip to on-disk values and non-sentinel values persist as new secrets
- [x] 2.3 Wire `PUT /admin/api/config/structured` into the shared spine: `base_hash` 409 guard → `from_config_with_upstream_and_recorder` validation → atomic write (temp + rename) → synchronous reload → restart-required warnings in response
- [x] 2.4 Register the route under the session-gated admin router; keep the text `PUT /admin/api/config` intact
- [x] 2.5 Tests: field edit preserves comments; added entry persists + activates; removed entry deleted; invalid config → 400 file unchanged; stale base_hash → 409 file unchanged; masked secret round-trips; changed secret persists

## 3. Secret reveal endpoint

- [x] 3.1 Implement `POST /admin/api/config/reveal`: resolve a single field reference (`providers.<id>.api_key`, `usage_database.url`, `gateway_keys.<index>`) against the current on-disk config, return its plaintext
- [x] 3.2 Enforce guards: session required (401), single field only, unknown reference → 404, non-secret reference → 400
- [x] 3.3 Register under the session-gated admin router
- [x] 3.4 Tests: reveal returns one secret and no other; no session → 401; unknown reference → 404; non-secret field → 400

## 4. Frontend structured form

- [x] 4.1 Add API client methods: `putConfigStructured(config, baseHash)` and `revealSecret(fieldRef)`; add typed structured-config interfaces mirroring the DTO
- [x] 4.2 Load config via existing `GET /admin/api/config`, parse masked TOML into the form's structured state (or add a structured GET shape if parsing TOML client-side proves brittle — decide during impl)
- [x] 4.3 Build the form shell in `ConfigPage.tsx`: sections for `gateway_keys`, `providers`, `routes`, `pricing`; add/remove repeatable entries; dirty tracking that never loses edits on failed save
- [x] 4.4 Provider editor: `id`, `protocol` (select), `base_url`, `api_key` (masked input + reveal/copy), `anthropic_version`, `model_aliases` (key/value editor)
- [x] 4.5 Route editor: `path_prefix`, `providers` (ordered multi-select from live provider ids, reorderable), `model_prefix`; surface dangling provider references (non-blocking)
- [x] 4.6 Pricing editor: `provider` (combobox of live ids + `*`), `model` (input with `model_aliases` suggestions + `*`), the per-1m decimal fields, `currency`, `pricing_source`
- [x] 4.7 Secret fields: masked by default, "copy" button calls reveal and writes to clipboard; edited value replaces on save; `admin_password` shown as static "set", `listen_addr`/`usage_database` absent
- [x] 4.8 Save flow: submit structured config to the new endpoint with base hash; success toast with restart-required warnings; 400 shows validation error and keeps edits; 409 offers "load current version"

## 5. Integration and verification

- [x] 5.1 Run `cargo build && cargo test`, fixing any failures
- [x] 5.2 Run `pnpm check` (tsc) and `pnpm build` in `ui/`, fixing any failures
- [x] 5.3 Rebuild the release binary so the updated SPA is embedded (`pnpm build` → `cargo build --release`)
- [x] 5.4 Update README/config docs: structured config editor, reveal-on-copy security note
