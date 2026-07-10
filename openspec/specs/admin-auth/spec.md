# admin-auth Specification

## Purpose
TBD - created by syncing change add-admin-ui. Update Purpose after archive.

## Requirements
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
The system SHALL verify login attempts by username and password against the user's argon2id hash, rejecting logins for unknown, disabled, or wrongly-authenticated users with an identical `401` response, and on success SHALL issue a session cookie with a random token of at least 128 bits of entropy, marked `HttpOnly` and `SameSite=Strict`, scoped to the `/admin` path, whose session records the user's id and role.

#### Scenario: Successful login
- **WHEN** a client sends `POST /admin/api/login` with a valid username and the correct password for an active user
- **THEN** the gateway MUST respond with success and set an `HttpOnly`, `SameSite=Strict` session cookie scoped to `/admin` whose session carries that user's identity and role.

#### Scenario: Failed login
- **WHEN** a client sends `POST /admin/api/login` with an unknown username, a disabled user's username, or an incorrect password
- **THEN** the gateway MUST respond `401 Unauthorized` with an identical body in all three cases and MUST NOT set a session cookie.

### Requirement: Admin API endpoints require a valid session
The system SHALL reject every `/admin/api/*` request except `POST /admin/api/login` with `401 Unauthorized` unless it carries a cookie referencing an unexpired session.

#### Scenario: Missing or invalid session cookie
- **WHEN** a client requests any `/admin/api/*` endpoint other than login without a valid session cookie
- **THEN** the gateway MUST respond `401 Unauthorized` without executing the endpoint.

#### Scenario: Expired session
- **WHEN** a client presents a session cookie whose session has exceeded its time-to-live
- **THEN** the gateway MUST respond `401 Unauthorized` and invalidate the session.

### Requirement: Failed logins are rate limited
The system SHALL reject login attempts for a username for a lockout window after 5 consecutive failed attempts for that username, regardless of the password submitted during the window, and MUST return the same `429` response whether or not the username exists.

#### Scenario: Lockout after consecutive failures
- **WHEN** 5 consecutive login attempts for a username fail and another attempt for that username arrives within the lockout window
- **THEN** the gateway MUST reject it with `429 Too Many Requests` even if the password is correct.

#### Scenario: Lockout expires
- **WHEN** the lockout window for a username has elapsed and a login attempt with the correct password arrives
- **THEN** the gateway MUST accept it and reset that username's failure counter.

#### Scenario: Lockout does not affect other users
- **WHEN** one username is locked out and a different user submits correct credentials
- **THEN** the gateway MUST accept the other user's login.

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

### Requirement: Sessions carry user identity and role
The system SHALL associate each session with the authenticated user's id and role, and SHALL reject requests to admin-only endpoints from `user`-role sessions with `403 Forbidden`. Self-service endpoints SHALL scope their effects and results to the session's user id.

#### Scenario: Non-admin blocked from admin endpoint
- **WHEN** a `user`-role session calls an admin-only endpoint such as user management or config
- **THEN** the system MUST respond `403 Forbidden` without executing the operation.

#### Scenario: Self-service scoped to the session user
- **WHEN** a `user`-role session lists API keys or usage
- **THEN** the response MUST include only data owned by the session's user.

#### Scenario: Disabled user's session is rejected
- **WHEN** a session's user is disabled after login
- **THEN** subsequent requests with that session MUST receive `401 Unauthorized`.

### Requirement: Sessions persist across restarts when Redis is configured
The system SHALL store sessions in Redis with their time-to-live when Redis is configured, so established sessions survive gateway process restarts; without Redis, sessions remain in-memory and restart behavior is unchanged. Login lockout state SHALL use the same backend selection.

#### Scenario: Session survives gateway restart with Redis
- **WHEN** the gateway restarts while Redis is configured and a client presents a session cookie issued before the restart and still within its TTL
- **THEN** the request MUST be authorized without a new login.

#### Scenario: In-memory sessions still die on restart
- **WHEN** the gateway restarts without Redis configured
- **THEN** previously issued session cookies MUST be rejected with `401`, as before.
