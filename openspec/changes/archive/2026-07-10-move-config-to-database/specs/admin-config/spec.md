# admin-config Specification (delta)

## MODIFIED Requirements

### Requirement: Config read returns masked TOML and a content hash
The system SHALL serve `GET /admin/api/config` returning the config file's TOML text with every secret value (`database.url`, `admin_password`) replaced by a `__MASKED__`-prefixed sentinel retaining at most the last 4 characters, plus a SHA-256 hash of the raw file bytes, while preserving the file's comments and formatting in the returned text. Dead sections left in the file after database migration (`providers`, `pricing`, `gateway_keys`) SHALL be returned as-is except that any secret values inside them MUST also be masked.

#### Scenario: Secrets are masked
- **WHEN** an authenticated admin requests `GET /admin/api/config`
- **THEN** the response MUST contain the TOML text with all secret values replaced by sentinels and MUST NOT contain any full secret value, including secrets inside dead sections.

#### Scenario: Content hash accompanies the text
- **WHEN** an authenticated admin requests `GET /admin/api/config`
- **THEN** the response MUST include the SHA-256 hash of the current on-disk file bytes.

### Requirement: Masked sentinels round-trip to current secret values
The system SHALL replace each `__MASKED__`-prefixed value in submitted config text with the corresponding current on-disk secret — `database.url` and `admin_password` matched by position — and SHALL reject a submission containing a sentinel it cannot unambiguously resolve with `400` naming the field.

#### Scenario: Untouched masked secret is preserved
- **WHEN** an authenticated admin submits config text in which `database.url` is still the masked sentinel
- **THEN** the persisted file MUST contain the current on-disk `database.url`.

#### Scenario: Replaced secret is persisted
- **WHEN** an authenticated admin submits config text in which a secret field carries a new non-sentinel value
- **THEN** the persisted file MUST contain the new value.

#### Scenario: Unresolvable sentinel is rejected
- **WHEN** submitted config text contains a masked sentinel for a field that does not exist in the current on-disk config
- **THEN** the gateway MUST respond `400` naming the unresolvable field and the config file MUST be unchanged.

### Requirement: Structured config write reconciles values into the existing TOML document
The system SHALL accept `PUT /admin/api/config/structured` carrying a structured configuration object covering the file-managed fields (`routes`, `retry`) plus the SHA-256 `base_hash` of the version it was based on, and SHALL apply the submitted values into the current on-disk TOML document in place — updating scalar values, adding entries for new array items, and removing entries for deleted ones — so that comments and formatting on surviving nodes are preserved. The endpoint MUST NOT accept provider, pricing, or gateway-key data. The endpoint MUST run the same masked-sentinel resolution, hot-reload validation (including route references against database providers), concurrency guard, atomic write, and reload as the text-based write.

#### Scenario: Route edit preserves surrounding comments
- **WHEN** an authenticated admin submits a structured config that changes one route's provider chain while the file has comments on other lines
- **THEN** the persisted file MUST contain the new chain and MUST retain the comments and formatting on all nodes that still exist.

#### Scenario: Structured write validates route references against database providers
- **WHEN** an authenticated admin submits a structured config containing a route that references a provider id not present in the database
- **THEN** the gateway MUST respond `400` with the validation error and the config file MUST be unchanged.

#### Scenario: Structured write detects concurrent modification
- **WHEN** the config file has changed on disk after the admin loaded it and the admin submits a structured config with the now-stale `base_hash`
- **THEN** the gateway MUST respond `409` with the current masked content and hash, and the config file MUST be unchanged.

### Requirement: Structured write preserves untouched masked secrets
The system SHALL treat a secret field (`database.url`) whose submitted value is still the `__MASKED__` sentinel as unchanged, resolving it to the corresponding current on-disk secret, and SHALL persist a non-sentinel value as the new secret.

#### Scenario: Masked secret round-trips to current value
- **WHEN** an authenticated admin submits a structured config in which `database.url` is still the masked sentinel
- **THEN** the persisted file MUST contain the current on-disk `database.url`.

#### Scenario: Changed secret is persisted
- **WHEN** an authenticated admin submits a structured config in which `database.url` carries a new non-sentinel value
- **THEN** the persisted file MUST contain the new value.

### Requirement: Single secret can be revealed on demand
The system SHALL serve `POST /admin/api/config/reveal` taking a reference to exactly one file-managed secret field (`database.url`) and returning that field's current plaintext value from the on-disk config. The endpoint MUST require an `admin`-role session, MUST reveal only the single referenced field, MUST respond `404` for a field reference that does not exist, and MUST respond `400` for a reference to a non-secret field. Provider key reveal is served by the provider management API, not this endpoint.

#### Scenario: Reveal returns a single secret
- **WHEN** an authenticated admin requests reveal of `database.url`
- **THEN** the response MUST contain the current plaintext value and MUST NOT contain any other secret value.

#### Scenario: Reveal requires an admin session
- **WHEN** a caller without an `admin`-role session requests the reveal endpoint
- **THEN** the gateway MUST respond `401` or `403` and MUST NOT return any secret value.

#### Scenario: Provider key reference is rejected here
- **WHEN** an authenticated admin requests reveal of a provider `api_key` through this endpoint
- **THEN** the gateway MUST respond `400` or `404` directing the caller to the provider management API.
