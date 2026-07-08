## Context

The admin console (`add-admin-ui`) shipped a config editor as a **raw TOML textarea** (CodeMirror). The `admin-config` capability is built around exchanging TOML *text*: `GET /admin/api/config` returns masked TOML + a SHA-256 hash; `PUT` takes TOML text, validates it, unmasks `__MASKED__` sentinels back to on-disk secrets, writes atomically (temp file + rename), and hot-reloads. `admin-ui` explicitly deferred structured editing to a later iteration.

This change fills that deferral: replace the textarea with a **structured form**, one control per config field, while keeping every safety property the text path established.

Relevant current-state facts:

- `config.rs` defines `GatewayConfig` with `listen_addr`, `gateway_keys[]`, `admin_password`, `providers[]`, `routes[]`, `usage_database`, `pricing[]`.
- Secrets masked today: `providers[].api_key`, `gateway_keys[]`, `usage_database.url`, `admin_password`. `GET` never emits a full secret.
- The existing `PUT` reconciles masked sentinels to current on-disk secrets, then runs the hot-reload validation path (`GatewayState::from_config_with_upstream_and_recorder`) before writing.
- `listen_addr` and `usage_database` are warn-only on reload (restart required); `admin_password` is a one-way argon2id hash written back at first boot.
- Config write access already equals provider-key custody (an editor can repoint `base_url` to steal keys), so the admin session is a trusted, fully-privileged boundary.

## Goals / Non-Goals

**Goals:**

- Replace the raw TOML editor with a structured form covering every hot-reloadable config field: `gateway_keys`, `providers` (incl. `model_aliases`), `routes`, `pricing`.
- Preserve user-authored TOML comments and formatting across a form save.
- Smart reference editing: `routes[].providers` and `pricing[].provider` are chosen from the live provider list, with the `*` wildcard preserved for pricing.
- Let an admin copy a real secret value (api_key / usage_database.url) to the clipboard on explicit demand.
- Keep the full text-path safety net: validate-before-write, atomic write, immediate reload, concurrency guard, masked-secret preservation.

**Non-Goals:**

- Editing `listen_addr` and `usage_database` in the form (restart-required; out of the form entirely for v1).
- Editing or changing `admin_password` via the UI (one-way hash; shown as "set", never editable).
- Removing the underlying TOML file format or the text-based `PUT` endpoint's masking model on `GET`.
- Multi-user config, per-field audit history, or config versioning/rollback.

## Decisions

### Decision 1: Form replaces the editor; save submits structured JSON, not TOML text

The CodeMirror textarea is removed. The form holds config as a typed JSON object and submits it to a **new structured write endpoint**. The frontend never serializes TOML.

**Rationale:** The two hard requirements — "form replaces editor" and "comments preserved" — together rule out the obvious paths. Serializing the form back to TOML in the browser loses comments; a whole-file JSON overwrite on the backend loses them too. Only backend reconciliation into the *existing* TOML document preserves them (Decision 2).

**Alternative considered — form + TOML dual view:** keep both, sync between them. Rejected: the user explicitly chose replacement, and two-way sync is a large, bug-prone surface for a lite tool.

### Decision 2: New `PUT /admin/api/config/structured` reconciles into the live `toml_edit` document

The new endpoint accepts a structured config object plus the same `base_hash` concurrency token. It loads the current on-disk TOML as a `toml_edit::DocumentMut` and **writes each field's value into the existing document** — updating scalars in place, adding table entries for new array items, removing tables for deleted ones — rather than regenerating the file.

```
form JSON + base_hash ──▶ load current TOML (toml_edit)
                          reconcile values into document (comments survive)
                          unmask secrets (reuse existing sentinel logic)
                          validate (from_config_...) ──▶ atomic write ──▶ reload
```

- **Comment survival scope:** comments attached to surviving nodes are preserved. A provider/route/pricing entry the admin deletes takes its comments with it (correct). A newly added entry has no comments (expected). Array reordering may shift `toml_edit` decor attribution — acceptable and documented.
- **Reuse:** masked-sentinel round-tripping, the `from_config_...` validation, atomic write, reload, and the SHA-256 `base_hash` 409 guard are all reused unchanged. Only the request shape (structured object vs text) and the reconcile step are new.
- The text `PUT /admin/api/config` is retained (no breaking removal); `GET` keeps returning masked TOML + hash so the concurrency token and secret model are unchanged.

**Alternative considered — extend the existing text `PUT` to also accept JSON:** overloading one endpoint on content type is muddier than a sibling route; a distinct path keeps each contract clean.

### Decision 3: Smart references with preserved wildcard

- `routes[].providers[]` — a multi-select over the live `providers[].id` list, ordered (order = failover priority), reorderable. Referencing a just-deleted provider is flagged in the UI (non-blocking; the backend still validates on save).
- `pricing[].provider` — a combobox of live provider ids **plus an explicit `*` (all providers)** option.
- `pricing[].model` — free text with suggestions drawn from the chosen provider's `model_aliases` values, plus an explicit `*` option; `provider="*"` + `model="*"` remains the global fallback price.

**Rationale:** choosing references from the real list eliminates the most common hand-editing error (typos in provider ids that only surface at save). The wildcard stays a first-class selectable option so pricing fallback semantics are fully retained.

### Decision 4: On-demand single-secret reveal endpoint (`POST /admin/api/config/reveal`)

`GET /admin/api/config` keeps masking every secret. A new session-gated endpoint returns **one** named secret's plaintext, only when the admin clicks "copy". The frontend writes it to the clipboard and discards it.

**Rationale:** the copy feature inherently requires plaintext in the browser — a masked sentinel can't be copied usefully. Rather than weaken `GET`'s "no full secret ever leaves the backend" property globally, reveal is scoped: session-required, one field per call, triggered only by an explicit user action. This mirrors AWS Console / Vault UI "reveal on click" patterns.

**Security trade-off (accepted by the user):** this is the one place the change reduces an existing property — plaintext secrets can now reach the browser. It is bounded by an already-fully-trusted admin session (config write already equals key custody), single-field scope, and explicit user intent.

**Field addressing:** reveal takes a stable field reference — `providers.<id>.api_key`, `usage_database.url`, or `gateway_keys.<index>` — resolved against the current on-disk config, returning `404` for an unknown reference and `400` for a non-secret field.

### Decision 5: The form is the single source of edit state; masked secrets stay masked until changed

Secret fields (`api_key`, `usage_database.url`, `gateway_keys[]`) render as masked inputs showing the sentinel. Leaving a field at its sentinel means "unchanged" (round-trips to the on-disk value via existing logic); typing a new value overrides it. A per-field "copy" button calls reveal (Decision 4). `admin_password` shows a static "set" indicator with no input.

## Risks / Trade-offs

- **Reveal endpoint exposes plaintext to the browser** → Mitigation: session-gated, single-field, explicit-action-only; bounded by the already-privileged admin boundary (accepted trade-off).
- **`toml_edit` reconcile is more complex than text replacement** → a structured diff into a live document has more edge cases (array reorder, nested tables) than the text path's whole-document reparse. Mitigation: reuse the existing unmask/validate/atomic-write/reload spine unchanged; only the reconcile step is new and it is unit-testable in isolation.
- **Comment attribution on reorder/delete** → `toml_edit` decor may not follow moved array entries perfectly. Mitigation: documented scope — comments survive for stable nodes; this is strictly better than the alternative (whole-file regeneration loses all comments).
- **Form and backend validation can drift** → the form's smart references encode reference rules (provider must exist, protocol consistency) that the backend also enforces. Mitigation: the backend `from_config_...` validation remains the single source of truth; the form's checks are advisory (better UX, non-authoritative), and save always revalidates server-side.
- **Two write endpoints (text + structured) to maintain** → minor duplication. Mitigation: both funnel into the shared unmask/validate/write/reload spine; only their request-decoding heads differ.
