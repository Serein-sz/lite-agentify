## ADDED Requirements

### Requirement: Gateway reserves the /admin path prefix
The system SHALL treat `/admin` and all subpaths as gateway-owned path space — served by the admin console when enabled, `404 Not Found` when disabled — and MUST NOT forward requests under `/admin` to any upstream provider.

#### Scenario: Admin path is never proxied
- **WHEN** a client requests any path under `/admin`, regardless of configured routes or authentication
- **THEN** the gateway MUST handle the request itself and MUST NOT contact any upstream provider.

#### Scenario: Non-admin paths are unaffected
- **WHEN** a client requests a path outside `/admin`, `/healthz`, and `/reload` that matches a configured route
- **THEN** the gateway MUST proxy it to the route's provider chain exactly as before this change.
