# admin-ui Specification (delta)

## ADDED Requirements

### Requirement: Console provides model catalog management
The console SHALL provide an admin-only model catalog view listing models (name, status, deployment count), supporting create, edit, enable/disable, and delete. Editing a model manages its ordered deployments — provider chosen from configured providers, upstream model name, drag-or-control reordering — and surfaces pricing-coverage errors (409) when enabling an uncovered model or mutating pricing-coupled state.

#### Scenario: Model with deployments is created and enabled
- **WHEN** an admin creates a model, adds priced deployments in order, and enables it
- **THEN** the catalog view MUST show the model enabled and clients MUST be able to call it without a restart.

#### Scenario: Enabling an uncovered model surfaces the pricing gate
- **WHEN** an admin enables a model that lacks pricing coverage for a deployment
- **THEN** the console MUST display the 409 error naming the uncovered deployment and the model MUST remain disabled.

#### Scenario: Deployment reordering
- **WHEN** an admin reorders a model's deployments and saves
- **THEN** the view MUST reflect the new order as failover priority.

## MODIFIED Requirements

### Requirement: Console provides self-service key management
The console SHALL provide a key management view where the user creates a key (with a name and an optional allowed-models selection picked from the enabled catalog, empty meaning all models), sees the plaintext exactly once in the creation result with a copy affordance and a warning that it cannot be shown again, and lists, edits the allowed-models of, and revokes keys (prefix, name, allowed models, status, created and last-used timestamps).

#### Scenario: Key creation shows plaintext once
- **WHEN** a user creates a key in the console
- **THEN** the console MUST display the full key once with a copy button and warn it will not be shown again, and the subsequent key list MUST show only the prefix and metadata.

#### Scenario: Key creation offers model selection
- **WHEN** a user creates or edits a key
- **THEN** the console MUST offer the enabled cataloged models as the allowed-models options, with no selection meaning all models.

#### Scenario: Key revocation
- **WHEN** a user revokes one of their keys in the console
- **THEN** the console MUST ask for confirmation, then show the key as revoked.

## REMOVED Requirements

### Requirement: Config editor edits configuration through a structured form
**Reason**: All form-editable configuration has moved to the database with dedicated management views (providers, pricing, models/routes, keys); the config file is reduced to hand-edited process/bootstrap settings.
**Migration**: Providers, pricing, and models are managed through their console views; `retry` is edited in the file (hot reload applies it).
