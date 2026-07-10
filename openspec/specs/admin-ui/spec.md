# admin-ui Specification

## Purpose
TBD - created by syncing change add-admin-ui. Update Purpose after archive.

## Requirements
### Requirement: Admin SPA is served from embedded assets under /admin
The system SHALL serve the admin single-page application under `/admin` from assets embedded in the release binary: `/admin` and any non-asset subpath return `index.html` (so client-side routes deep-link correctly) with `Cache-Control: no-cache`, and hashed build assets are served with their correct MIME type and an immutable cache policy.

#### Scenario: Entry page
- **WHEN** a browser requests `/admin` on a gateway with the admin console enabled
- **THEN** the gateway MUST return the SPA's `index.html` with a `no-cache` cache policy.

#### Scenario: Client-side route deep link
- **WHEN** a browser directly requests a client-side route path such as `/admin/config`
- **THEN** the gateway MUST return `index.html` so the SPA router can render the route.

#### Scenario: Static asset with correct type
- **WHEN** a browser requests an existing hashed asset such as `/admin/assets/index-<hash>.js`
- **THEN** the gateway MUST return the asset bytes with a JavaScript MIME type and an immutable cache policy.

#### Scenario: Single-binary deployment
- **WHEN** a release build of the gateway binary is copied to a host without the `ui/` directory and started with the admin console enabled
- **THEN** the admin SPA MUST be fully served from the binary with no runtime file dependencies.

### Requirement: Unauthenticated console access lands on login
The console SHALL present a login view with username and password fields to unauthenticated visitors, and after successful login SHALL present the role-appropriate landing view; API responses of `401` SHALL return the user to the login view.

#### Scenario: Visit without session
- **WHEN** a visitor without a valid session opens `/admin`
- **THEN** the console MUST show the login view with username and password fields and MUST NOT render usage or config data.

#### Scenario: Session expiry during use
- **WHEN** a user's session expires and a console API call returns `401`
- **THEN** the console MUST return to the login view.

### Requirement: Dashboard visualizes usage
The console's dashboard SHALL display, for a selectable time range: summary cards (request count, token totals, cost, average latency, error rate), a cost/token time-series chart, a per-provider/model breakdown, and a paginated, filterable request log backed by the usage endpoints.

#### Scenario: Dashboard renders summary data
- **WHEN** an authenticated admin opens the dashboard with usage recording enabled
- **THEN** the console MUST render summary cards, a time-series chart, a breakdown view, and the request log from the usage API for the selected range.

#### Scenario: Dashboard with usage disabled
- **WHEN** an authenticated admin opens the dashboard and the usage API reports `usage_enabled: false`
- **THEN** the console MUST show an explicit "usage recording disabled" state instead of empty charts or an error.

### Requirement: Console is partitioned by role
The console SHALL render views according to the session's role: `user`-role sessions see key management, their own usage dashboard, and password change; `admin`-role sessions additionally see user management, all-users usage, and the config editor. Views a role cannot access MUST NOT be reachable through navigation, and direct navigation to them MUST be denied client-side with the API enforcing `403` server-side.

#### Scenario: User-role navigation
- **WHEN** a `user`-role account logs into the console
- **THEN** the console MUST show key management, own usage, and password change, and MUST NOT offer user management or the config editor.

#### Scenario: Admin-role navigation
- **WHEN** an `admin`-role account logs into the console
- **THEN** the console MUST offer user management, usage across all users, key management, and the config editor.

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

### Requirement: Console provides admin user management
The console SHALL provide an admin-only user management view listing users with role and status, supporting user creation (username, initial password, role), disable/enable, and password reset.

#### Scenario: Admin creates a user
- **WHEN** an admin creates a user in the console with a username, initial password, and role
- **THEN** the console MUST show the new user in the list as active.

#### Scenario: Admin disables a user
- **WHEN** an admin disables a user in the console
- **THEN** the console MUST ask for confirmation and then show the user as disabled.

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

### Requirement: Console provides admin credit management
The console SHALL provide an admin-only credit view listing users with granted total, spent, and balance, supporting granting credit (amount, note) and negative corrections with confirmation, and showing each user's ledger history.

#### Scenario: Admin grants credit
- **WHEN** an admin grants an amount to a user with a note
- **THEN** the console MUST show the updated balance and the new ledger entry.

#### Scenario: Correction requires confirmation
- **WHEN** an admin enters a negative amount
- **THEN** the console MUST ask for confirmation before submitting.

### Requirement: Console shows balance and spend to every user
The console SHALL display the signed-in user's remaining balance, granted total, and cumulative spend on their dashboard, and SHALL show per-key spent-to-date (and cap, when set) in the key management view with a cap-edit control.

#### Scenario: User sees their balance
- **WHEN** a `user`-role account opens the dashboard
- **THEN** the console MUST display their remaining balance, granted total, and cumulative spend.

#### Scenario: Key list shows spend against cap
- **WHEN** a user views their keys and one key has a cap
- **THEN** the console MUST show that key's spent-to-date together with its cap, and allow editing the cap.
