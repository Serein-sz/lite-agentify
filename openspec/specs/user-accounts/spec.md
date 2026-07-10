# user-accounts Specification

## Purpose
TBD - created by syncing change add-user-account-system. Update Purpose after archive.

## Requirements
### Requirement: User accounts are stored in PostgreSQL with roles and status
The system SHALL store user accounts in a PostgreSQL `users` table with a unique username, an argon2id password hash, a role of `admin` or `user`, and a status of `active` or `disabled`. The system MUST NOT store plaintext passwords.

#### Scenario: User record fields
- **WHEN** an admin creates a user with a username and initial password
- **THEN** the persisted record MUST contain the username, an argon2id PHC hash of the password, the assigned role, and `active` status, and MUST NOT contain the plaintext password.

#### Scenario: Duplicate username is rejected
- **WHEN** an admin creates a user whose username already exists
- **THEN** the system MUST reject the request with a conflict error and MUST NOT modify the existing user.

### Requirement: PostgreSQL is a mandatory dependency
The system SHALL require a configured, reachable PostgreSQL database at startup, run its schema migrations before serving, and fail startup with a clear error when the database is unreachable or migration fails.

#### Scenario: Startup without a database
- **WHEN** the gateway starts without a configured database or the database is unreachable
- **THEN** the gateway MUST log a clear error naming the database requirement and exit instead of serving requests.

#### Scenario: Migrations run before serving
- **WHEN** the gateway starts against a database missing account tables
- **THEN** the gateway MUST create the required schema via migrations before accepting requests.

### Requirement: First boot seeds a bootstrap admin from config
The system SHALL, when the `users` table is empty at startup, create an `admin`-role user named `admin` whose password is taken from the `admin_password` config value, reusing the existing plaintext-to-argon2id write-back behavior. When the `users` table is non-empty the `admin_password` config value MUST NOT create or modify any user.

#### Scenario: Empty users table seeds admin
- **WHEN** the gateway starts with an empty `users` table and `admin_password` configured
- **THEN** the system MUST create an active `admin` user verifying against that password.

#### Scenario: Existing users are never overwritten by config
- **WHEN** the gateway starts with a non-empty `users` table
- **THEN** the system MUST NOT create or alter any user from the `admin_password` config value.

#### Scenario: Empty users table without admin_password
- **WHEN** the gateway starts with an empty `users` table and no `admin_password` configured
- **THEN** the gateway MUST fail startup with an error explaining that a bootstrap admin password is required.

### Requirement: Admins manage user lifecycle
The system SHALL provide admin-session-only endpoints to create users (username, initial password, role), disable and re-enable users, and reset a user's password. Disabling a user MUST invalidate the user's sessions and MUST cause all of the user's API keys to stop authenticating no later than the next snapshot refresh.

#### Scenario: Admin creates a user
- **WHEN** an admin submits a new username, initial password, and role
- **THEN** the system MUST create the active user and the user MUST be able to log in with that password.

#### Scenario: Disabled user loses access
- **WHEN** an admin disables a user
- **THEN** the user's sessions MUST become invalid, login attempts MUST be rejected, and requests bearing the user's API keys MUST be rejected after the snapshot refresh.

#### Scenario: Non-admin cannot manage users
- **WHEN** a `user`-role session calls any user-management endpoint
- **THEN** the system MUST respond `403 Forbidden` without performing the operation.

### Requirement: Users can change their own password
The system SHALL allow an authenticated user to change their own password by presenting the current password, and MUST reject the change when the current password does not verify.

#### Scenario: Password change with correct current password
- **WHEN** an authenticated user submits their current password and a new password
- **THEN** the system MUST update the stored hash and subsequent logins MUST verify against the new password only.

#### Scenario: Password change with wrong current password
- **WHEN** an authenticated user submits an incorrect current password
- **THEN** the system MUST respond `401` and MUST NOT change the stored hash.
