## MODIFIED Requirements

### Requirement: Config editor edits configuration through a structured form
The console SHALL provide a config editor that presents the gateway configuration as a structured form — editing `gateway_keys`, `providers`, `routes`, and `pricing` as typed fields and repeatable entries rather than raw text — and on save MUST submit the structured configuration to the config API, surfacing validation errors (400), conflict responses (409, offering to load the fresh version), and restart-required warnings without losing the admin's unsaved edits. Reference fields SHALL be chosen from existing values: a route's providers and a pricing rule's provider are selected from the configured providers, while the wildcard `*` remains available for pricing. Restart-only settings (`listen_addr`, `usage_database`) and `admin_password` SHALL NOT be editable in the form. Secret fields SHALL display as masked, revealing or copying the real value only on explicit request, and an edited value SHALL replace the secret on save.

#### Scenario: Successful save
- **WHEN** an authenticated admin edits fields in the form and saves a valid configuration
- **THEN** the console MUST report that the configuration was saved and reloaded, and display any restart-required warnings from the response.

#### Scenario: Validation error preserves edits
- **WHEN** an authenticated admin saves a configuration that the API rejects with a validation error
- **THEN** the console MUST display the error and keep the admin's edited form state intact.

#### Scenario: Conflict offers refresh
- **WHEN** a save is rejected with `409` because the file changed on disk
- **THEN** the console MUST inform the admin and offer to load the current version, without silently discarding the admin's edits.

#### Scenario: Provider reference is chosen from configured providers
- **WHEN** an authenticated admin adds a provider to a route or sets a pricing rule's provider
- **THEN** the form MUST offer the configured providers as the selectable options, and for pricing MUST also offer the wildcard `*`.

#### Scenario: Deleting a referenced provider is surfaced
- **WHEN** an authenticated admin removes a provider that a route still references
- **THEN** the form MUST indicate the dangling reference before save, and the API MUST still reject an invalid configuration on submit.

#### Scenario: Secret stays masked until revealed
- **WHEN** an authenticated admin views a secret field such as a provider's `api_key`
- **THEN** the form MUST display it masked, and MUST fetch the real value only when the admin explicitly reveals or copies it.

#### Scenario: Restart-only and password fields are absent
- **WHEN** an authenticated admin opens the config editor
- **THEN** the form MUST NOT present `listen_addr`, `usage_database`, or `admin_password` as editable fields.
