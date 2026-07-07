# config-hot-reload Specification

## Purpose

Reload gateway configuration at runtime without restarting the process: atomic snapshot swap, file-watch and endpoint triggers, keep-previous-config failure semantics, and warnings for non-reloadable fields.

## Requirements

### Requirement: Gateway reloads configuration without restart

The system SHALL support reloading the gateway configuration file at runtime and SHALL apply reloaded provider, route, model alias, pricing, and gateway key configuration to subsequent requests without restarting the gateway process.

#### Scenario: Reloaded provider configuration serves subsequent requests

- **WHEN** the gateway configuration file is modified to change provider, route, model alias, pricing, or gateway key settings and a reload completes successfully
- **THEN** the gateway MUST route and process subsequent requests using the reloaded configuration.

#### Scenario: In-flight requests are unaffected by reload

- **WHEN** a configuration reload completes while a request is being processed
- **THEN** the gateway MUST finish processing that request with the configuration snapshot it started with, and MUST NOT interrupt or fail the request because of the reload.

### Requirement: Gateway keeps serving with previous configuration when reload fails

The system SHALL validate reloaded configuration with the same rules as startup validation, and on any read, parse, or validation failure SHALL keep the previously active configuration serving traffic and log the failure. The system MUST NOT apply a partially loaded configuration.

#### Scenario: Invalid TOML keeps old configuration

- **WHEN** a reload is triggered and the configuration file contains invalid TOML
- **THEN** the gateway MUST continue serving requests with the previously active configuration and MUST log the parse error.

#### Scenario: Configuration failing validation keeps old configuration

- **WHEN** a reload is triggered and the configuration file parses but fails validation (for example a route referencing an unknown provider or an empty gateway key list)
- **THEN** the gateway MUST continue serving requests with the previously active configuration and MUST log the validation error.

#### Scenario: Reload is atomic

- **WHEN** a reload succeeds
- **THEN** the gateway MUST switch all reloadable configuration (providers, routes, model aliases, pricing, gateway keys) to the new snapshot as a single atomic replacement, and MUST NOT serve any request with a mixture of old and new reloadable configuration.

### Requirement: Gateway excludes listen address and usage database from hot reload

The system SHALL NOT apply changes to the listen address or usage database configuration during a reload. When a reload detects changes to these fields, the system SHALL log a warning stating a restart is required and SHALL still apply the remaining reloadable configuration.

#### Scenario: Changed listen address is ignored with a warning

- **WHEN** a reload is triggered and the configuration file changes `listen_addr`
- **THEN** the gateway MUST keep listening on the original address, MUST log a warning that the change requires a restart, and MUST apply the other reloadable configuration changes.

#### Scenario: Changed usage database settings are ignored with a warning

- **WHEN** a reload is triggered and the configuration file changes the usage database configuration
- **THEN** the gateway MUST keep the existing usage recording setup, MUST log a warning that the change requires a restart, and MUST apply the other reloadable configuration changes.

### Requirement: Gateway watches the configuration file for changes

The system SHALL watch the configuration file for modifications using a cross-platform file notification mechanism and SHALL trigger a reload when the file changes. The system SHALL debounce rapid successive file events into a single reload attempt.

#### Scenario: Saving the configuration file triggers a reload

- **WHEN** the configuration file is modified on disk while the gateway is running
- **THEN** the gateway MUST attempt a configuration reload without manual intervention.

#### Scenario: Rapid successive writes are debounced

- **WHEN** the configuration file receives multiple write events within the debounce window (for example an editor writing a temporary file and renaming it)
- **THEN** the gateway MUST coalesce them into a single reload attempt.

#### Scenario: Watcher failure does not affect proxying

- **WHEN** the file watcher cannot be created or stops delivering events
- **THEN** the gateway MUST log the watcher failure and MUST continue serving proxy traffic, and manual reload via the reload endpoint MUST remain available.

### Requirement: Gateway exposes an authenticated reload endpoint

The system SHALL expose a `POST /reload` endpoint that requires gateway key authentication, triggers a configuration reload, and reports the reload outcome in the response.

#### Scenario: Authenticated reload request applies new configuration

- **WHEN** a client sends `POST /reload` with a valid gateway key and the configuration file is valid
- **THEN** the gateway MUST reload the configuration and return a success response.

#### Scenario: Reload endpoint reports failure reason

- **WHEN** a client sends `POST /reload` with a valid gateway key and the configuration file fails to load or validate
- **THEN** the gateway MUST keep the previously active configuration, and MUST return an error response describing the failure without exposing secret values.

#### Scenario: Unauthenticated reload request is rejected

- **WHEN** a client sends `POST /reload` without a valid gateway key
- **THEN** the gateway MUST reject the request and MUST NOT trigger a reload.

### Requirement: Gateway key changes take effect on reload

The system SHALL apply reloaded gateway keys to authentication of subsequent requests, so that newly added keys are accepted and removed keys are rejected immediately after a successful reload.

#### Scenario: Newly added gateway key is accepted

- **WHEN** a reload adds a gateway key and a client subsequently authenticates with that key
- **THEN** the gateway MUST accept the request.

#### Scenario: Removed gateway key is rejected

- **WHEN** a reload removes a gateway key and a client subsequently authenticates with the removed key
- **THEN** the gateway MUST reject the request as unauthenticated.
