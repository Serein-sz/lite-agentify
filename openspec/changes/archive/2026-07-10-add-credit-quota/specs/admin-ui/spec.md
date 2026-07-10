# admin-ui Specification (delta)

## ADDED Requirements

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
