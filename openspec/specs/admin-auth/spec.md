# admin-auth Specification

## Purpose
TBD - created by syncing change add-admin-ui. Update Purpose after archive.

## Requirements
### Requirement: Admin console is enabled only by a configured admin password
The system SHALL serve the admin console and admin API only when the optional top-level `admin_password` config field is set, and SHALL respond `404 Not Found` to all `/admin` paths when it is not set.

#### Scenario: Admin disabled without password
- **WHEN** the gateway runs with no `admin_password` configured and a client requests any path under `/admin`
- **THEN** the gateway MUST respond `404 Not Found` and MUST NOT forward the request upstream.

#### Scenario: Admin enabled with password
- **WHEN** the gateway runs with `admin_password` configured and a client requests `/admin`
- **THEN** the gateway MUST serve the admin console entry page.

### Requirement: Plaintext admin password is hashed and written back at startup
The system SHALL detect a plaintext `admin_password` value at startup (a value not in PHC `$argon2id$` format), replace it in the config file with its argon2id PHC hash while preserving the file's comments and formatting, and perform this before the config file watcher starts.

#### Scenario: First boot with plaintext password
- **WHEN** the gateway starts and `admin_password` contains a plaintext value
- **THEN** the gateway MUST rewrite the config file with the argon2id PHC hash of that value, preserving all other content including comments, and subsequent logins MUST verify against the hash.

#### Scenario: Boot with already-hashed password
- **WHEN** the gateway starts and `admin_password` already contains a `$argon2id$` PHC string
- **THEN** the gateway MUST NOT modify the config file.

#### Scenario: Write-back failure does not prevent startup
- **WHEN** the config file cannot be rewritten (e.g. read-only filesystem)
- **THEN** the gateway MUST log a prominent warning, continue startup, and verify logins against the in-memory hash of the plaintext value.

### Requirement: Login issues a hardened session cookie
The system SHALL verify login attempts against the argon2id hash and, on success, issue a session cookie with a random token of at least 128 bits of entropy, marked `HttpOnly` and `SameSite=Strict`, scoped to the `/admin` path.

#### Scenario: Successful login
- **WHEN** a client sends `POST /admin/api/login` with the correct password
- **THEN** the gateway MUST respond with success and set an `HttpOnly`, `SameSite=Strict` session cookie scoped to `/admin`.

#### Scenario: Failed login
- **WHEN** a client sends `POST /admin/api/login` with an incorrect password
- **THEN** the gateway MUST respond `401 Unauthorized` and MUST NOT set a session cookie.

### Requirement: Admin API endpoints require a valid session
The system SHALL reject every `/admin/api/*` request except `POST /admin/api/login` with `401 Unauthorized` unless it carries a cookie referencing an unexpired session.

#### Scenario: Missing or invalid session cookie
- **WHEN** a client requests any `/admin/api/*` endpoint other than login without a valid session cookie
- **THEN** the gateway MUST respond `401 Unauthorized` without executing the endpoint.

#### Scenario: Expired session
- **WHEN** a client presents a session cookie whose session has exceeded its time-to-live
- **THEN** the gateway MUST respond `401 Unauthorized` and invalidate the session.

### Requirement: Failed logins are rate limited
The system SHALL reject all login attempts for a lockout window after 5 consecutive failed attempts, regardless of the password submitted during the window.

#### Scenario: Lockout after consecutive failures
- **WHEN** 5 consecutive login attempts fail and another login attempt arrives within the lockout window
- **THEN** the gateway MUST reject it with `429 Too Many Requests` even if the password is correct.

#### Scenario: Lockout expires
- **WHEN** the lockout window has elapsed and a login attempt with the correct password arrives
- **THEN** the gateway MUST accept it and reset the failure counter.

### Requirement: Logout invalidates the session
The system SHALL provide `POST /admin/api/logout` that invalidates the presented session.

#### Scenario: Logout
- **WHEN** an authenticated client sends `POST /admin/api/logout`
- **THEN** the gateway MUST invalidate that session, and subsequent requests with the same cookie MUST receive `401 Unauthorized`.

### Requirement: Sessions survive configuration reloads
The system SHALL keep established admin sessions valid across hot reloads of the gateway configuration.

#### Scenario: Reload does not log admins out
- **WHEN** the gateway configuration is hot-reloaded while an admin session is active
- **THEN** subsequent requests with that session cookie MUST remain authorized.
