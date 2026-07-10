# api-key-management Specification

## Purpose
TBD - created by syncing change add-user-account-system. Update Purpose after archive.

## Requirements
### Requirement: API keys are stored hashed with a display prefix
The system SHALL store API keys in an `api_keys` table containing the SHA-256 hash of the key, a display prefix of at most 8 characters, a user-chosen name, the owning user id, a status of `active` or `revoked`, and creation/last-used timestamps. The system MUST NOT store the plaintext key.

#### Scenario: Key record contains no plaintext
- **WHEN** a key is created
- **THEN** the persisted record MUST contain the SHA-256 hash and display prefix and MUST NOT contain the full plaintext key.

### Requirement: Users create their own API keys with one-time plaintext reveal
The system SHALL allow any authenticated user to create an API key owned by themselves, generating a server-side random token with at least 192 bits of entropy prefixed `la-`, and SHALL return the plaintext key exactly once in the creation response. No other endpoint may return the plaintext.

#### Scenario: Key creation returns plaintext once
- **WHEN** an authenticated user creates a key
- **THEN** the response MUST contain the full plaintext key, and subsequent listings MUST show only the prefix, name, status, and timestamps.

#### Scenario: Key authenticates proxy requests
- **WHEN** a client presents a newly created key on a proxied request
- **THEN** the gateway MUST authenticate the request and attribute it to the owning user and key.

### Requirement: Users list and revoke their own keys; admins see all keys
The system SHALL allow a user to list and revoke only their own keys, and an admin to list and revoke any key. Revocation MUST be permanent and MUST stop the key authenticating no later than the next snapshot refresh.

#### Scenario: User lists own keys only
- **WHEN** a `user`-role session lists keys
- **THEN** the response MUST contain only keys owned by that user.

#### Scenario: Revoked key stops authenticating
- **WHEN** a key is revoked
- **THEN** proxied requests presenting that key MUST be rejected with `401` after the snapshot refresh, and the key MUST NOT be re-activatable.

#### Scenario: User cannot revoke another user's key
- **WHEN** a `user`-role session attempts to revoke a key it does not own
- **THEN** the system MUST respond `403` or `404` and the key MUST remain active.

### Requirement: Key lookups stay off the request hot path
The system SHALL resolve presented API keys against an in-process snapshot keyed by SHA-256 hash, rebuilt from the database at startup and refreshed after any key or user mutation, and MUST NOT query the database per proxied request for authentication.

#### Scenario: Key mutation refreshes the snapshot
- **WHEN** a key is created or revoked, or a user is disabled
- **THEN** the in-process snapshot MUST be refreshed so the change takes effect without a process restart.

#### Scenario: No per-request database authentication
- **WHEN** a proxied request is authenticated
- **THEN** the gateway MUST resolve the key from the in-process snapshot without a database round trip.

### Requirement: Legacy gateway_keys are imported once as admin-owned keys
The system SHALL, when the `api_keys` table is empty at startup and the config file contains `gateway_keys`, import each entry as an active API key owned by the bootstrap admin (storing its SHA-256 hash, named `imported-<n>`), and SHALL thereafter ignore the `gateway_keys` config field, logging a warning when it is present.

#### Scenario: Legacy keys keep working after upgrade
- **WHEN** the gateway starts for the first time after the upgrade with existing `gateway_keys` in config
- **THEN** clients presenting those keys MUST continue to authenticate, attributed to the bootstrap admin.

#### Scenario: Import happens only once
- **WHEN** the gateway restarts with a non-empty `api_keys` table
- **THEN** the system MUST NOT import `gateway_keys` again, even if the config field is still present.

### Requirement: API keys can restrict callable models
The system SHALL support an optional `allowed_models` list on an API key; when present, requests with that key for a model outside the list MUST be rejected with `403` before any upstream contact, and when absent the key may call any enabled cataloged model. The restriction SHALL be resolved from the in-process snapshot without a per-request database query. Key owners set the list at creation and may edit it on their own keys.

#### Scenario: Key restricted to a model set
- **WHEN** a client presents a key whose `allowed_models` is `["model-a"]` and requests `model-b`
- **THEN** the gateway MUST respond `403` naming the restriction and MUST NOT contact any provider.

#### Scenario: Unrestricted key calls any cataloged model
- **WHEN** a client presents a key without `allowed_models` and requests any enabled cataloged model
- **THEN** the gateway MUST resolve and forward the request normally.

#### Scenario: Restriction referencing a removed model is inert
- **WHEN** a key's `allowed_models` names a model that no longer exists in the catalog
- **THEN** requests for that name MUST fail as unknown-model, and the key's other allowed models MUST keep working.

### Requirement: API keys carry an optional spend cap and expose spent-to-date
The system SHALL support an optional cumulative USD spend cap on an API key, settable at creation and editable afterwards by the key's owner or an admin, and SHALL include each key's cumulative attributed cost in key listings. Cap enforcement semantics are defined in `credit-quota`.

#### Scenario: Owner sets a cap at creation
- **WHEN** a user creates a key with a 5 USD cap
- **THEN** the persisted key MUST carry the cap and the key listing MUST show the cap alongside spent-to-date.

#### Scenario: Cap is editable later
- **WHEN** a key owner raises the cap on an existing key
- **THEN** subsequent enforcement MUST use the new cap without recreating the key.
