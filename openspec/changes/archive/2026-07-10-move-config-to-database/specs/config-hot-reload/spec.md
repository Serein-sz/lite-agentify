# config-hot-reload Specification (delta)

## MODIFIED Requirements

### Requirement: Gateway reloads configuration without restart

The system SHALL support reloading the gateway configuration file at runtime and SHALL apply reloaded route and retry configuration to subsequent requests without restarting the gateway process. Provider, pricing, user, and API key configuration live in the database and take effect through snapshot rebuilds triggered by their management APIs, not file reload; a file reload MUST still re-validate routes against the current database providers.

#### Scenario: Reloaded route configuration serves subsequent requests

- **WHEN** the gateway configuration file is modified to change route or retry settings and a reload completes successfully
- **THEN** the gateway MUST route and process subsequent requests using the reloaded configuration.

#### Scenario: In-flight requests are unaffected by reload

- **WHEN** a configuration reload completes while a request is being processed
- **THEN** the gateway MUST finish processing that request with the configuration snapshot it started with, and MUST NOT interrupt or fail the request because of the reload.

#### Scenario: Database mutation refreshes the snapshot without file reload

- **WHEN** an admin mutates providers, pricing, users, or API keys through the management APIs
- **THEN** the gateway MUST rebuild and atomically swap the snapshot without requiring a config file change.

### Requirement: Gateway keeps serving with previous configuration when reload fails

The system SHALL validate reloaded configuration with the same rules as startup validation — including route references against database providers — and on any read, parse, or validation failure SHALL keep the previously active configuration serving traffic and log the failure. The system MUST NOT apply a partially loaded configuration.

#### Scenario: Invalid TOML keeps old configuration

- **WHEN** a reload is triggered and the configuration file contains invalid TOML
- **THEN** the gateway MUST continue serving requests with the previously active configuration and MUST log the parse error.

#### Scenario: Configuration failing validation keeps old configuration

- **WHEN** a reload is triggered and the configuration file parses but fails validation (for example a route referencing a provider id not present in the database)
- **THEN** the gateway MUST continue serving requests with the previously active configuration and MUST log the validation error.

#### Scenario: Reload is atomic

- **WHEN** a reload succeeds
- **THEN** the gateway MUST switch all snapshot state (database-sourced and file-sourced alike) to the new snapshot as a single atomic replacement, and MUST NOT serve any request with a mixture of old and new configuration.

### Requirement: Gateway excludes listen address and usage database from hot reload

The system SHALL NOT apply changes to the listen address or database connection configuration during a reload. When a reload detects changes to these fields, the system SHALL log a warning stating a restart is required and SHALL still apply the remaining reloadable configuration.

#### Scenario: Changed listen address is ignored with a warning

- **WHEN** a reload is triggered and the configuration file changes `listen_addr`
- **THEN** the gateway MUST keep listening on the original address, MUST log a warning that the change requires a restart, and MUST apply the other reloadable configuration changes.

#### Scenario: Changed database settings are ignored with a warning

- **WHEN** a reload is triggered and the configuration file changes the `database` configuration
- **THEN** the gateway MUST keep the existing database connection, MUST log a warning that the change requires a restart, and MUST apply the other reloadable configuration changes.

### Requirement: Gateway exposes an authenticated reload endpoint

The system SHALL expose a `POST /reload` endpoint that requires API key authentication, triggers a configuration reload, and reports the reload outcome in the response.

#### Scenario: Authenticated reload request applies new configuration

- **WHEN** a client sends `POST /reload` with a valid API key and the configuration file is valid
- **THEN** the gateway MUST reload the configuration and return a success response.

#### Scenario: Reload endpoint reports failure reason

- **WHEN** a client sends `POST /reload` with a valid API key and the configuration file fails to load or validate
- **THEN** the gateway MUST keep the previously active configuration, and MUST return an error response describing the failure without exposing secret values.

#### Scenario: Unauthenticated reload request is rejected

- **WHEN** a client sends `POST /reload` without a valid API key
- **THEN** the gateway MUST reject the request and MUST NOT trigger a reload.

## REMOVED Requirements

### Requirement: Gateway key changes take effect on reload
**Reason**: Static `gateway_keys` were replaced by database-backed API keys in `add-user-account-system`; key changes take effect through snapshot rebuilds triggered by the key management API, not file reload.
**Migration**: Create and revoke keys through `/admin/api/keys`; no file edit or reload is involved.
