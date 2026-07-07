# admin-config Specification

## Purpose
TBD - created by syncing change add-admin-ui. Update Purpose after archive.

## Requirements
### Requirement: Config read returns masked TOML and a content hash
The system SHALL serve `GET /admin/api/config` returning the config file's TOML text with every secret value (`providers[].api_key`, `gateway_keys[]` entries, `usage_database.url`, `admin_password`) replaced by a `__MASKED__`-prefixed sentinel retaining at most the last 4 characters, plus a SHA-256 hash of the raw file bytes, while preserving the file's comments and formatting in the returned text.

#### Scenario: Secrets are masked
- **WHEN** an authenticated admin requests `GET /admin/api/config`
- **THEN** the response MUST contain the TOML text with all secret values replaced by sentinels and MUST NOT contain any full secret value.

#### Scenario: Content hash accompanies the text
- **WHEN** an authenticated admin requests `GET /admin/api/config`
- **THEN** the response MUST include the SHA-256 hash of the current on-disk file bytes.

### Requirement: Config write validates before persisting
The system SHALL accept `PUT /admin/api/config` with TOML text and MUST validate it — TOML parse, config deserialization, and gateway state construction (the same validation as hot reload) — before any file modification, rejecting invalid submissions with `400` and the validation error while leaving the file unchanged.

#### Scenario: Syntactically invalid TOML is rejected
- **WHEN** an authenticated admin submits config text that fails TOML parsing
- **THEN** the gateway MUST respond `400` with the parse error and the config file MUST be unchanged.

#### Scenario: Semantically invalid config is rejected
- **WHEN** an authenticated admin submits config text that parses but fails gateway validation (e.g. a route referencing an unknown provider)
- **THEN** the gateway MUST respond `400` with the validation error and the config file MUST be unchanged.

### Requirement: Masked sentinels round-trip to current secret values
The system SHALL replace each `__MASKED__`-prefixed value in submitted config text with the corresponding current on-disk secret — `providers[].api_key` matched by provider `id`, `usage_database.url` and `admin_password` by position, `gateway_keys[]` by index only when the list length is unchanged — and SHALL reject a submission containing a sentinel it cannot unambiguously resolve with `400` naming the field.

#### Scenario: Untouched masked secret is preserved
- **WHEN** an authenticated admin submits config text in which an existing provider's `api_key` is still the masked sentinel
- **THEN** the persisted file MUST contain that provider's current on-disk `api_key`.

#### Scenario: Replaced secret is persisted
- **WHEN** an authenticated admin submits config text in which a secret field carries a new non-sentinel value
- **THEN** the persisted file MUST contain the new value.

#### Scenario: Unresolvable sentinel is rejected
- **WHEN** submitted config text contains a masked `api_key` under a provider `id` that does not exist in the current on-disk config
- **THEN** the gateway MUST respond `400` naming the unresolvable field and the config file MUST be unchanged.

### Requirement: Config write persists atomically and takes effect immediately
The system SHALL persist a validated config by writing a temporary file in the config directory and renaming it over the original, then reload the gateway state before responding, reporting success together with any restart-required warnings (`listen_addr`, `usage_database` changes).

#### Scenario: Valid write takes effect without restart
- **WHEN** an authenticated admin submits valid config text with a matching base hash
- **THEN** the gateway MUST persist it atomically, activate the new configuration for subsequent requests without a process restart, and respond with success.

#### Scenario: Restart-required change is warned
- **WHEN** a valid submission changes `listen_addr` or `usage_database`
- **THEN** the response MUST include a warning that the change requires a restart to take effect.

### Requirement: Concurrent config modification is detected
The system SHALL require `PUT /admin/api/config` to carry the SHA-256 hash of the file version the submission was based on, and MUST reject the write with `409` — returning the current file text and hash — when the on-disk file no longer matches.

#### Scenario: Stale base hash is rejected
- **WHEN** the config file has been modified (e.g. by hand) after the admin loaded it, and the admin submits changes with the now-stale base hash
- **THEN** the gateway MUST respond `409` with the current masked content and hash, and the config file MUST be unchanged.
