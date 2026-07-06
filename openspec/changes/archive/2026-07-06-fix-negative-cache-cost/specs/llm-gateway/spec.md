## MODIFIED Requirements

### Requirement: Gateway estimates cost with cache-aware pricing
The system SHALL calculate estimated cost from configured provider/model pricing and MUST account for cached token classes separately from regular input tokens. The system MUST subtract from regular input tokens only cache token classes that the provider reports as a subset of input tokens, MUST NOT subtract cache token classes the provider reports independently of input tokens, and MUST NOT produce a negative estimated cost.

#### Scenario: OpenAI-compatible cached prompt tokens are priced separately
- **WHEN** OpenAI-compatible usage metadata includes cached prompt tokens
- **THEN** the gateway MUST subtract cached prompt tokens from regular input tokens before applying regular input pricing and MUST apply cached input pricing to cached prompt tokens when configured.

#### Scenario: Anthropic-compatible cache tokens are priced separately
- **WHEN** Anthropic-compatible usage metadata includes cache creation or cache read input tokens
- **THEN** the gateway MUST NOT subtract cache creation or cache read input tokens from regular input tokens, because the provider reports them independently of input tokens, and MUST apply cache write and cache read pricing to those token classes when configured.

#### Scenario: Cache usage lacks cache pricing
- **WHEN** usage metadata includes cache token counts but the matching pricing entry lacks the required cache price
- **THEN** the gateway MUST persist token counts and MUST leave estimated cost unavailable rather than charging cache tokens as regular input tokens.

#### Scenario: Cache read tokens exceed reported input tokens
- **WHEN** usage metadata reports cache read or cache creation tokens that exceed the reported input tokens
- **THEN** the gateway MUST estimate a non-negative cost and MUST NOT reduce regular input tokens below zero.
