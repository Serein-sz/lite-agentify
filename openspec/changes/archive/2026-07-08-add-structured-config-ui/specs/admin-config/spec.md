## ADDED Requirements

### Requirement: Structured config write reconciles values into the existing TOML document
The system SHALL accept `PUT /admin/api/config/structured` carrying a structured configuration object plus the SHA-256 `base_hash` of the version it was based on, and SHALL apply the submitted values into the current on-disk TOML document in place — updating scalar values, adding entries for new array items, and removing entries for deleted ones — so that comments and formatting on surviving nodes are preserved. The endpoint MUST run the same masked-sentinel resolution, hot-reload validation, concurrency guard, atomic write, and reload as the text-based write.

#### Scenario: Field edit preserves surrounding comments
- **WHEN** an authenticated admin submits a structured config that changes one provider's `base_url` while the file has comments on other lines
- **THEN** the persisted file MUST contain the new `base_url` and MUST retain the comments and formatting on all nodes that still exist.

#### Scenario: Added array entry is persisted
- **WHEN** an authenticated admin submits a structured config containing a new provider not present on disk
- **THEN** the persisted file MUST contain the new provider entry and the configuration MUST activate without a process restart.

#### Scenario: Removed array entry is deleted
- **WHEN** an authenticated admin submits a structured config that omits a provider previously present on disk
- **THEN** the persisted file MUST NOT contain that provider entry after the write.

#### Scenario: Structured write validates before persisting
- **WHEN** an authenticated admin submits a structured config that parses but fails gateway validation (e.g. a route referencing an unknown provider)
- **THEN** the gateway MUST respond `400` with the validation error and the config file MUST be unchanged.

#### Scenario: Structured write detects concurrent modification
- **WHEN** the config file has changed on disk after the admin loaded it and the admin submits a structured config with the now-stale `base_hash`
- **THEN** the gateway MUST respond `409` with the current masked content and hash, and the config file MUST be unchanged.

### Requirement: Structured write preserves untouched masked secrets
The system SHALL treat a secret field (`providers[].api_key`, `gateway_keys[]`, `usage_database.url`) whose submitted value is still the `__MASKED__` sentinel as unchanged, resolving it to the corresponding current on-disk secret, and SHALL persist a non-sentinel value as the new secret.

#### Scenario: Masked secret round-trips to current value
- **WHEN** an authenticated admin submits a structured config in which a provider's `api_key` is still the masked sentinel
- **THEN** the persisted file MUST contain that provider's current on-disk `api_key`.

#### Scenario: Changed secret is persisted
- **WHEN** an authenticated admin submits a structured config in which a provider's `api_key` carries a new non-sentinel value
- **THEN** the persisted file MUST contain the new value.

### Requirement: Single secret can be revealed on demand
The system SHALL serve `POST /admin/api/config/reveal` taking a reference to exactly one secret field (`providers.<id>.api_key`, `usage_database.url`, or `gateway_keys.<index>`) and returning that field's current plaintext value from the on-disk config. The endpoint MUST require a valid admin session, MUST reveal only the single referenced field, MUST respond `404` for a field reference that does not exist, and MUST respond `400` for a reference to a non-secret field.

#### Scenario: Reveal returns a single secret
- **WHEN** an authenticated admin requests reveal of `providers.<id>.api_key` for an existing provider
- **THEN** the response MUST contain that provider's current plaintext `api_key` and MUST NOT contain any other secret value.

#### Scenario: Reveal requires a session
- **WHEN** a caller without a valid admin session requests the reveal endpoint
- **THEN** the gateway MUST respond `401` and MUST NOT return any secret value.

#### Scenario: Unknown field reference is rejected
- **WHEN** an authenticated admin requests reveal of a field reference that does not exist in the current config (e.g. a provider id that is not configured)
- **THEN** the gateway MUST respond `404` and MUST NOT return any secret value.

#### Scenario: Non-secret field reference is rejected
- **WHEN** an authenticated admin requests reveal of a field that is not a secret
- **THEN** the gateway MUST respond `400` and MUST NOT return any value.
