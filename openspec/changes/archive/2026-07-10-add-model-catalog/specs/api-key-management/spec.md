# api-key-management Specification (delta)

## ADDED Requirements

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
