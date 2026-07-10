# provider-management Specification (delta)

## ADDED Requirements

### Requirement: Providers are stored in PostgreSQL as the source of truth
The system SHALL store provider configuration (id, protocol, base URL, upstream API key, optional Anthropic version, model aliases) in a PostgreSQL `providers` table, and SHALL build the in-process snapshot's provider set from the database, applying the same validation as today's startup checks (unique non-empty id, base URL with scheme and host, non-empty API key, non-empty alias mappings).

#### Scenario: Snapshot serves database providers
- **WHEN** the gateway builds its state snapshot
- **THEN** the provider set MUST come from the `providers` table and proxied requests MUST route to those providers.

#### Scenario: Invalid provider row fails the rebuild safely
- **WHEN** a snapshot rebuild encounters an invalid provider record
- **THEN** the gateway MUST keep serving with the previous snapshot and log the validation error.

### Requirement: Admins manage providers through a CRUD API
The system SHALL provide admin-session-only endpoints to list, create, update, and delete providers. Write operations MUST validate the submitted provider, MUST reject deleting a provider still referenced by a configured route with `409` naming the reference, and MUST trigger an in-process snapshot rebuild after commit so changes take effect without a restart.

#### Scenario: Created provider serves traffic without restart
- **WHEN** an admin creates a valid provider and a subsequent snapshot rebuild completes
- **THEN** routes referencing that provider MUST be able to forward requests to it without a process restart.

#### Scenario: Deleting a referenced provider is rejected
- **WHEN** an admin deletes a provider that a route still references
- **THEN** the system MUST respond `409` naming the referencing route and the provider MUST remain.

#### Scenario: Non-admin cannot manage providers
- **WHEN** a `user`-role session calls a provider management endpoint
- **THEN** the system MUST respond `403 Forbidden`.

### Requirement: Provider upstream keys are masked with single-secret reveal
The system SHALL mask the provider `api_key` in every list/read response (a `__MASKED__` sentinel retaining at most the last 4 characters), SHALL treat a submitted masked sentinel on update as "keep the current value", and SHALL provide an admin-only endpoint revealing exactly one named provider's current key, responding `404` for an unknown provider.

#### Scenario: Listing masks all keys
- **WHEN** an admin lists providers
- **THEN** every `api_key` value in the response MUST be masked and no full key may appear.

#### Scenario: Update with sentinel keeps the current key
- **WHEN** an admin updates a provider whose submitted `api_key` is still the masked sentinel
- **THEN** the stored key MUST remain unchanged.

#### Scenario: Reveal returns one key to an admin only
- **WHEN** an admin requests reveal for one provider id
- **THEN** the response MUST contain only that provider's plaintext key; a non-admin session MUST receive `403` and an unknown id MUST receive `404`.

### Requirement: File providers are imported once
The system SHALL, when the `providers` table is empty at startup and the config file contains `[[providers]]` entries, import them (including model aliases) into the database in one transaction, and SHALL thereafter ignore the file section, logging a warning while it remains present.

#### Scenario: First boot imports file providers
- **WHEN** the gateway starts with an empty `providers` table and providers in the config file
- **THEN** the database MUST contain those providers and proxying MUST behave as before the upgrade.

#### Scenario: File section is dead after import
- **WHEN** the gateway starts with a non-empty `providers` table and the file still contains `[[providers]]`
- **THEN** the file entries MUST NOT be applied and a warning MUST be logged.
