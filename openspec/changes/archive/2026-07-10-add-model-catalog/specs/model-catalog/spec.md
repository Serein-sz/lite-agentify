# model-catalog Specification (delta)

## ADDED Requirements

### Requirement: Models are cataloged with ordered provider deployments
The system SHALL store a model catalog in PostgreSQL: each model has a unique public name and `enabled`/`disabled` status, and an ordered list of deployments, each referencing an existing provider with a non-empty upstream model name; deployment order expresses failover priority. Deleting a provider referenced by a deployment MUST be rejected with `409` naming the model.

#### Scenario: Model with ordered deployments
- **WHEN** an admin creates a model with deployments `[provider-a: name-x, provider-b: name-y]`
- **THEN** the persisted catalog MUST preserve that order as the failover priority.

#### Scenario: Deployment referencing unknown provider is rejected
- **WHEN** an admin submits a deployment referencing a provider id that does not exist
- **THEN** the system MUST reject the mutation and the catalog MUST be unchanged.

#### Scenario: Provider referenced by a deployment cannot be deleted
- **WHEN** an admin deletes a provider that a model deployment references
- **THEN** the system MUST respond `409` naming the model and the provider MUST remain.

### Requirement: Requests resolve through the model catalog
The system SHALL resolve every provider pass-through request by looking up the request body's top-level `model` in the catalog, filtering the model's deployments to providers whose protocol matches the request endpoint's protocol family, and attempting the filtered chain in order with existing failover and retry semantics, rewriting the top-level `model` to each attempted deployment's upstream model name. Unknown and disabled models MUST be rejected with a protocol-native error naming the model, before any upstream contact.

#### Scenario: Cataloged model routes along its deployment chain
- **WHEN** an authenticated client sends `POST /v1/messages` with a cataloged enabled model whose protocol-matching deployments are `[primary, fallback]`
- **THEN** the gateway MUST attempt primary first with its upstream model name, failing over to fallback (with fallback's upstream name) under the existing failover conditions.

#### Scenario: Unknown model is rejected without upstream contact
- **WHEN** an authenticated client sends a request whose `model` is not in the catalog
- **THEN** the gateway MUST return a protocol-native error naming the model and MUST NOT contact any provider.

#### Scenario: Disabled model is rejected
- **WHEN** an authenticated client requests a model whose catalog status is `disabled`
- **THEN** the gateway MUST return a protocol-native error and MUST NOT contact any provider.

#### Scenario: Protocol filter excludes non-matching deployments
- **WHEN** a model's deployments span OpenAI-protocol and Anthropic-protocol providers and a request arrives on an Anthropic-family endpoint
- **THEN** the gateway MUST attempt only the Anthropic-protocol deployments, in catalog order.

#### Scenario: No protocol-matching deployment yields a clear error
- **WHEN** a cataloged model has no deployment matching the request endpoint's protocol family
- **THEN** the gateway MUST return an error naming the endpoint family and the families the model supports, without contacting any provider.

### Requirement: Enabled models require pricing coverage
The system SHALL require that every deployment of an `enabled` model resolves to a pricing rule through the established wildcard fallback order, enforcing this when a model is enabled, when deployments of an enabled model are mutated, and when pricing mutations would remove coverage; violating mutations MUST be rejected with `409` naming the model. Disabled models are exempt.

#### Scenario: Enabling an unpriced model is rejected
- **WHEN** an admin enables a model having a deployment whose provider+upstream model resolves no pricing rule
- **THEN** the system MUST reject with `409` naming the uncovered deployment and the model MUST stay disabled.

#### Scenario: Pricing deletion that strips coverage is rejected
- **WHEN** an admin deletes a pricing rule that is the only coverage for a deployment of an enabled model
- **THEN** the system MUST respond `409` naming the model and the rule MUST remain.

### Requirement: Admins manage the catalog through a CRUD API
The system SHALL provide admin-session-only endpoints to list, create, update, enable/disable, and delete models and their deployments, triggering a snapshot rebuild after commit so changes apply without restart.

#### Scenario: Catalog change applies without restart
- **WHEN** an admin reorders a model's deployments and the snapshot rebuild completes
- **THEN** subsequent requests for that model MUST follow the new order.

#### Scenario: Non-admin cannot manage the catalog
- **WHEN** a `user`-role session calls a catalog management endpoint
- **THEN** the system MUST respond `403 Forbidden`.

### Requirement: Gateway serves /v1/models from the catalog
The system SHALL answer `GET /v1/models` itself with the enabled cataloged models available to the presented API key, rendered in the protocol-native shape of the endpoint family, and MUST NOT forward the request upstream.

#### Scenario: Model list is key-scoped
- **WHEN** a client whose key restricts `allowed_models` requests `GET /v1/models`
- **THEN** the response MUST list exactly the enabled models the key may use and MUST NOT be forwarded upstream.

### Requirement: Routes and provider aliases migrate to the catalog once
The system SHALL, when the `models` table is empty at startup and the config file contains routes, convert each route chain's provider `model_aliases` into catalog models and ordered deployments; models lacking pricing coverage are created `disabled`. Providers in chains without aliases MUST be reported in a startup warning as requiring manual catalog entries. Thereafter the file `routes` section and provider alias data are ignored, with a warning while present.

#### Scenario: Alias-bearing route converts to catalog entries
- **WHEN** the gateway first boots with an empty catalog and a file route `[provider-a, provider-b]` where both providers alias `public-x`
- **THEN** the catalog MUST contain model `public-x` with deployments for provider-a and provider-b in route order.

#### Scenario: Unpriced migrated model starts disabled
- **WHEN** migration creates a model whose deployments lack pricing coverage
- **THEN** the model MUST be created with `disabled` status and reported in the startup log.

#### Scenario: Migration runs only once
- **WHEN** the gateway restarts with a non-empty `models` table
- **THEN** the migration MUST NOT run again even if the file still contains routes.
