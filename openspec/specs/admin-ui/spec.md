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
The console SHALL present a login view to unauthenticated visitors, and after successful login SHALL present the dashboard; API responses of `401` SHALL return the user to the login view.

#### Scenario: Visit without session
- **WHEN** a visitor without a valid session opens `/admin`
- **THEN** the console MUST show the login view and MUST NOT render usage or config data.

#### Scenario: Session expiry during use
- **WHEN** an admin's session expires and a console API call returns `401`
- **THEN** the console MUST return to the login view.

### Requirement: Dashboard visualizes usage
The console's dashboard SHALL display, for a selectable time range: summary cards (request count, token totals, cost, average latency, error rate), a cost/token time-series chart, a per-provider/model breakdown, and a paginated, filterable request log backed by the usage endpoints.

#### Scenario: Dashboard renders summary data
- **WHEN** an authenticated admin opens the dashboard with usage recording enabled
- **THEN** the console MUST render summary cards, a time-series chart, a breakdown view, and the request log from the usage API for the selected range.

#### Scenario: Dashboard with usage disabled
- **WHEN** an authenticated admin opens the dashboard and the usage API reports `usage_enabled: false`
- **THEN** the console MUST show an explicit "usage recording disabled" state instead of empty charts or an error.

### Requirement: Config editor edits raw TOML safely
The console SHALL provide a config editor showing the masked TOML with syntax highlighting, and on save MUST surface validation errors (400), conflict responses (409, offering to load the fresh version), and restart-required warnings returned by the config API without losing the admin's unsaved text.

#### Scenario: Successful save
- **WHEN** an authenticated admin saves valid config text
- **THEN** the console MUST report that the configuration was saved and reloaded, and display any restart-required warnings from the response.

#### Scenario: Validation error preserves the draft
- **WHEN** an authenticated admin saves config text that the API rejects with a validation error
- **THEN** the console MUST display the error and keep the admin's edited text intact.

#### Scenario: Conflict offers refresh
- **WHEN** a save is rejected with `409` because the file changed on disk
- **THEN** the console MUST inform the admin and offer to load the current version, without silently discarding the admin's edits.
