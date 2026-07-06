## ADDED Requirements

### Requirement: Gateway resolves provider-specific model aliases
The system SHALL allow each provider to define model aliases that map public client-facing model names to provider-specific upstream model names.

#### Scenario: Request model is rewritten for the selected provider
- **WHEN** an authenticated client sends a provider pass-through request whose top-level `model` value matches an alias configured for the selected provider
- **THEN** the gateway MUST forward the request to that provider with the top-level `model` value replaced by the configured upstream model name.

#### Scenario: Fallback provider receives its own mapped model
- **WHEN** an authenticated request matches a route with a failover chain and the gateway attempts a fallback provider after the primary fails
- **THEN** the gateway MUST resolve the original client-facing model name against the fallback provider's aliases before forwarding the fallback request.

### Requirement: Gateway preserves existing model pass-through when aliases are absent
The system SHALL preserve current provider pass-through behavior for providers that do not configure model aliases.

#### Scenario: Provider without aliases receives original model
- **WHEN** an authenticated client sends a provider pass-through request to a matched route and the selected provider has no model aliases configured
- **THEN** the gateway MUST forward the request body without changing the top-level `model` value.

### Requirement: Gateway does not expose unresolved upstream model names through alias-enabled providers
The system SHALL NOT forward a request to a provider with configured model aliases unless the request's top-level `model` value resolves through that provider's alias map.

#### Scenario: Provider without requested alias is skipped before upstream contact
- **WHEN** an authenticated request matches a route and a provider in the route chain has model aliases configured but does not define the requested model alias
- **THEN** the gateway MUST NOT contact that provider for the request.

#### Scenario: Later provider can serve requested alias
- **WHEN** an authenticated request matches a route whose first provider cannot resolve the requested model alias and a later provider can resolve it
- **THEN** the gateway MUST forward the request to the later provider with that provider's configured upstream model name.

#### Scenario: No provider can resolve requested alias
- **WHEN** an authenticated request matches a route but every alias-enabled provider in the route chain lacks the requested model alias
- **THEN** the gateway MUST return a gateway error without contacting those providers.

### Requirement: Gateway limits alias rewriting to protocol-native model fields
The system SHALL only rewrite the top-level string `model` field in JSON request bodies and MUST NOT otherwise transform protocol-native request or response schemas.

#### Scenario: Only top-level model field changes
- **WHEN** an authenticated request body contains a top-level string `model` value that resolves through the selected provider's aliases
- **THEN** the gateway MUST preserve all other request body fields unchanged when forwarding upstream.

#### Scenario: Responses remain provider-native
- **WHEN** an upstream provider returns a response to a request that used model alias resolution
- **THEN** the gateway MUST forward the provider-native response without rewriting response body model fields.
