## Why

The admin console's config editor is a raw TOML textarea: operators hand-edit text, and reference mistakes (a route naming a provider `id` that doesn't exist, a misspelled protocol) only surface as a `400` after saving. A structured form that draws provider/route/pricing references from the actual configured values eliminates that class of error at the source and makes the full configuration editable without knowing TOML.

## What Changes

- **BREAKING (UI only)**: The config page replaces the CodeMirror raw-TOML editor with a structured form covering the hot-reloadable config: `gateway_keys`, `providers`, `routes`, and `pricing`. No API is removed; the raw-text `PUT /admin/api/config` stays for backward compatibility.
- Reference fields become guided selections instead of free text: a route's `providers` are chosen from the configured provider list (ordered = failover priority); `pricing.provider` is chosen from that list plus an explicit `*` wildcard option; `pricing.model` offers the provider's alias targets plus `*`. Wildcard semantics are preserved.
- New **structured write endpoint** accepting a JSON config object: the backend reconciles it into the existing on-disk TOML document via `toml_edit`, changing only value nodes so comments and formatting on surviving keys are preserved, then runs the same validate → atomic-write → reload path as the text editor.
- New **secret reveal endpoint**: on an explicit per-field request (the form's "copy" action), the backend returns a single secret's plaintext (`providers[].api_key`, `usage_database.url`) so the operator can copy it. `GET /admin/api/config` remains fully masked — plaintext is never sent except through this deliberate, single-field, session-gated request.
- `listen_addr`, `usage_database`, and `admin_password` are **out of the form**: the first two require a restart (edit the file directly), and `admin_password` is a one-way hash with no UI edit path. `usage_database.url` still participates in reveal/copy.
- Masked secret fields (`api_key`, `usage_database.url`, `gateway_keys[]`) render as masked inputs that keep the current value unless the operator types a replacement — the same round-trip contract the text editor already relies on.

## Capabilities

### New Capabilities
<!-- none -->

### Modified Capabilities
- `admin-config`: Adds a structured (JSON) config write endpoint that reconciles into the existing TOML document preserving comments, and a session-gated single-field secret reveal endpoint; the existing masked read, validation, atomic write, reload, and conflict-detection requirements are unchanged and reused.
- `admin-ui`: The "config editor" requirement changes from a raw-TOML CodeMirror editor to a structured form over the hot-reloadable config, with guided reference selection (providers/routes/pricing), masked secret inputs with copy-to-clipboard, and the same save/validation/conflict handling.

## Impact

- Affected code: `src/admin/config_api.rs` (new structured PUT + reconcile logic, new reveal handler), `src/admin/mod.rs` (route wiring), `src/config.rs` (a serializable view of the config for the structured payload). Frontend: `ui/src/pages/ConfigPage.tsx` rewritten as a form; new form/subform components; `ui/src/api.ts` (structured PUT + reveal client). CodeMirror and its theme dependency are removed from the config page.
- No new Rust dependencies: `toml_edit`, `serde`, and the existing masking/validation/atomic-write helpers cover it.
- Security: one deliberate reduction — the reveal endpoint returns a single plaintext secret to an authenticated session on explicit request. Accepted because config-write access already implies key custody; mirrors AWS/Vault "reveal on demand". Default masking on `GET /config` is unchanged.
- Out of scope: editing `listen_addr`/`usage_database`/`admin_password` via UI; multiple config profiles; import/export.
