# admin-ui Specification (delta)

## ADDED Requirements

### Requirement: Console provides provider management
The console SHALL provide an admin-only provider management view listing providers (id, protocol, base URL, masked key, alias count), supporting create, edit, and delete with validation errors surfaced inline. Secret fields SHALL display masked, fetching the plaintext only on explicit reveal/copy; an edited value replaces the secret on save. Deleting a provider that a route still references MUST surface the API's conflict error.

#### Scenario: Provider CRUD without restart
- **WHEN** an admin creates or edits a provider in the console and saves successfully
- **THEN** the view MUST reflect the change and the configuration MUST be active without a process restart.

#### Scenario: Secret stays masked until revealed
- **WHEN** an admin views a provider's `api_key`
- **THEN** the console MUST display it masked and fetch the plaintext only on explicit reveal or copy.

#### Scenario: Referenced provider deletion is surfaced
- **WHEN** an admin attempts to delete a provider that a route references
- **THEN** the console MUST display the conflict error naming the reference and the provider MUST remain listed.

### Requirement: Console provides pricing management
The console SHALL provide an admin-only pricing management view listing rules (provider, model, rates, currency), supporting create, edit, and delete, with provider chosen from configured providers or the `*` wildcard and model as free text or `*`.

#### Scenario: Pricing rule edit applies without restart
- **WHEN** an admin edits a pricing rule and saves successfully
- **THEN** the view MUST reflect the change and subsequent cost estimation MUST use the new rate without a process restart.

#### Scenario: Provider reference options
- **WHEN** an admin sets a pricing rule's provider
- **THEN** the console MUST offer the configured providers and the `*` wildcard as options.

## MODIFIED Requirements

### Requirement: Config editor edits configuration through a structured form
The console SHALL provide a config editor that presents the file-managed gateway configuration as a structured form — editing `routes` as typed fields and repeatable entries rather than raw text — and on save MUST submit the structured configuration to the config API, surfacing validation errors (400), conflict responses (409, offering to load the fresh version), and restart-required warnings without losing the admin's unsaved edits. A route's providers SHALL be selected from the database-managed providers (order = failover priority). Providers, pricing, and API keys SHALL NOT appear in this editor (they have dedicated management views); restart-only settings (`listen_addr`, `database`) and `admin_password` SHALL NOT be editable in the form. The editor SHALL label routes as file-managed pending their replacement by the model catalog.

#### Scenario: Successful save
- **WHEN** an authenticated admin edits route fields in the form and saves a valid configuration
- **THEN** the console MUST report that the configuration was saved and reloaded, and display any restart-required warnings from the response.

#### Scenario: Validation error preserves edits
- **WHEN** an authenticated admin saves a configuration that the API rejects with a validation error
- **THEN** the console MUST display the error and keep the admin's edited form state intact.

#### Scenario: Conflict offers refresh
- **WHEN** a save is rejected with `409` because the file changed on disk
- **THEN** the console MUST inform the admin and offer to load the current version, without silently discarding the admin's edits.

#### Scenario: Route providers are chosen from database providers
- **WHEN** an authenticated admin adds a provider to a route
- **THEN** the form MUST offer the database-managed providers as the selectable options.

#### Scenario: Provider and pricing sections are absent
- **WHEN** an authenticated admin opens the config editor
- **THEN** the form MUST NOT present provider, pricing, gateway-key, `listen_addr`, `database`, or `admin_password` fields.
