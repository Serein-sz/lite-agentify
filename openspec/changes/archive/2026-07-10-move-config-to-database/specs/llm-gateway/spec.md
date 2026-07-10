# llm-gateway Specification (delta)

## MODIFIED Requirements

### Requirement: Gateway configures usage persistence and pricing externally
The system SHALL read database connectivity from gateway configuration (the mandatory `database` section) and provider/model pricing from the database's pricing rules. The gateway MUST NOT rely on hard-coded provider model prices.

#### Scenario: Database configuration is required
- **WHEN** the gateway starts
- **THEN** it MUST initialize PostgreSQL persistence through SeaORM using the configured `database` section, and MUST fail startup when it is absent or unreachable.

#### Scenario: Pricing rules are loaded from the database
- **WHEN** the gateway builds its snapshot and pricing rules exist in the database
- **THEN** it MUST use those rules for cost estimation and MUST NOT rely on hard-coded provider model prices.

#### Scenario: Pricing falls back through explicit wildcards
- **WHEN** no exact pricing rule matches the selected provider id and upstream model
- **THEN** the gateway MUST look for pricing in this order: selected provider id with `*` model, `*` provider with upstream model, and `*` provider with `*` model.

#### Scenario: Example configuration is documented
- **WHEN** a user reviews the gateway configuration documentation or sample configuration
- **THEN** the system MUST include examples for the PostgreSQL `database` section and document that pricing is managed through the admin console or API.
