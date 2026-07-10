# api-key-management Specification (delta)

## ADDED Requirements

### Requirement: API keys carry an optional spend cap and expose spent-to-date
The system SHALL support an optional cumulative USD spend cap on an API key, settable at creation and editable afterwards by the key's owner or an admin, and SHALL include each key's cumulative attributed cost in key listings. Cap enforcement semantics are defined in `credit-quota`.

#### Scenario: Owner sets a cap at creation
- **WHEN** a user creates a key with a 5 USD cap
- **THEN** the persisted key MUST carry the cap and the key listing MUST show the cap alongside spent-to-date.

#### Scenario: Cap is editable later
- **WHEN** a key owner raises the cap on an existing key
- **THEN** subsequent enforcement MUST use the new cap without recreating the key.
