# config-hot-reload Specification (delta)

## MODIFIED Requirements

### Requirement: Gateway reloads configuration without restart

The system SHALL support reloading the gateway configuration file at runtime and SHALL apply reloaded retry configuration to subsequent requests without restarting the gateway process. Providers, pricing, models, users, and API keys live in the database and take effect through snapshot rebuilds triggered by their management APIs, not file reload.

#### Scenario: Reloaded retry configuration serves subsequent requests

- **WHEN** the gateway configuration file is modified to change `retry` settings and a reload completes successfully
- **THEN** the gateway MUST apply the reloaded retry policy to subsequent requests.

#### Scenario: In-flight requests are unaffected by reload

- **WHEN** a configuration reload completes while a request is being processed
- **THEN** the gateway MUST finish processing that request with the configuration snapshot it started with, and MUST NOT interrupt or fail the request because of the reload.

#### Scenario: Database mutation refreshes the snapshot without file reload

- **WHEN** an admin mutates providers, pricing, models, users, or API keys through the management APIs
- **THEN** the gateway MUST rebuild and atomically swap the snapshot without requiring a config file change.
