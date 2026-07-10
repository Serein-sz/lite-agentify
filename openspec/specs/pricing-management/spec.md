# pricing-management Specification

## Purpose
TBD - created by syncing change move-config-to-database. Update Purpose after archive.

## Requirements
### Requirement: Pricing rules are stored in PostgreSQL as the source of truth
The system SHALL store pricing rules (provider or `*`, model or `*`, per-million token rates including cache variants, currency, optional pricing source) in a PostgreSQL `pricing` table, SHALL build the snapshot's pricing map from the database, and SHALL preserve today's lookup order: provider+model, provider+`*`, `*`+model, `*`+`*`.

#### Scenario: Cost estimation uses database pricing
- **WHEN** a proxied request completes and pricing rules exist for the provider/model
- **THEN** the estimated cost MUST be computed from the database-stored rates using the established wildcard fallback order.

#### Scenario: Duplicate rule is rejected
- **WHEN** an admin creates a pricing rule whose provider+model pair already exists
- **THEN** the system MUST reject it with a conflict error.

### Requirement: Admins manage pricing rules through a CRUD API
The system SHALL provide admin-session-only endpoints to list, create, update, and delete pricing rules, validating rates as non-negative decimals, and MUST trigger a snapshot rebuild after commit so changes apply to subsequent requests without a restart.

#### Scenario: Updated rate applies without restart
- **WHEN** an admin updates a pricing rule and the snapshot rebuild completes
- **THEN** subsequent requests MUST be costed with the new rate.

#### Scenario: Non-admin cannot manage pricing
- **WHEN** a `user`-role session calls a pricing management endpoint
- **THEN** the system MUST respond `403 Forbidden`.

### Requirement: File pricing entries are imported once
The system SHALL, when the `pricing` table is empty at startup and the config file contains `[[pricing]]` entries, import them into the database in one transaction, and SHALL thereafter ignore the file section, logging a warning while it remains present.

#### Scenario: First boot imports file pricing
- **WHEN** the gateway starts with an empty `pricing` table and pricing entries in the config file
- **THEN** the database MUST contain those rules and cost estimation MUST behave as before the upgrade.

#### Scenario: File section is dead after import
- **WHEN** the gateway starts with a non-empty `pricing` table and the file still contains `[[pricing]]`
- **THEN** the file entries MUST NOT be applied and a warning MUST be logged.
