# llm-gateway Specification (delta)

## MODIFIED Requirements

### Requirement: Gateway routes support an ordered provider failover chain
The system SHALL attempt a resolved model's protocol-filtered deployments in catalog order, where order expresses priority, until one returns a non-failover response or the chain is exhausted.

#### Scenario: Primary provider success skips fallback providers
- **WHEN** an authenticated request resolves to a deployment chain `[primary, fallback]` and the primary returns a non-failover response
- **THEN** the gateway MUST forward the primary response to the client and MUST NOT contact the fallback provider.

#### Scenario: Primary transport failure falls over to next provider
- **WHEN** an authenticated request resolves to a deployment chain `[primary, fallback]` and the primary request fails with a transport error
- **THEN** the gateway MUST retry the same request against the fallback deployment and forward the fallback response.

#### Scenario: Primary 5xx response falls over to next provider
- **WHEN** an authenticated request resolves to a deployment chain `[primary, fallback]` and the primary returns an HTTP 5xx status
- **THEN** the gateway MUST retry the same request against the fallback deployment and forward the fallback response.

#### Scenario: Exhausted failover chain returns a gateway error
- **WHEN** every deployment in the resolved chain fails with a transport error or HTTP 5xx status
- **THEN** the gateway MUST return a gateway error response after the last attempt.

### Requirement: Gateway does not convert between provider protocols
The system MUST preserve protocol-native request and response formats and MUST NOT translate OpenAI-compatible requests into Anthropic-compatible requests or Anthropic-compatible requests into OpenAI-compatible requests. Deployment chains are filtered to the request endpoint's protocol family rather than translated.

#### Scenario: OpenAI-compatible request never falls through to Anthropic conversion
- **WHEN** an authenticated OpenAI-compatible request resolves to a model whose only deployments are Anthropic-protocol providers
- **THEN** the gateway MUST return a resolution error instead of converting the request to Anthropic-compatible format.

#### Scenario: Anthropic-compatible request never falls through to OpenAI conversion
- **WHEN** an authenticated Anthropic-compatible request resolves to a model whose only deployments are OpenAI-protocol providers
- **THEN** the gateway MUST return a resolution error instead of converting the request to OpenAI-compatible format.

### Requirement: Gateway limits alias rewriting to protocol-native model fields
The system SHALL only rewrite the top-level string `model` field in JSON request bodies — replacing the public catalog name with the attempted deployment's upstream model name — and MUST NOT otherwise transform protocol-native request or response schemas.

#### Scenario: Only top-level model field changes
- **WHEN** an authenticated request body contains a top-level string `model` value resolved through the catalog
- **THEN** the gateway MUST preserve all other request body fields unchanged when forwarding upstream.

#### Scenario: Responses remain provider-native
- **WHEN** an upstream provider returns a response to a request whose model was rewritten to the deployment's upstream name
- **THEN** the gateway MUST forward the provider-native response without rewriting response body model fields.

## REMOVED Requirements

### Requirement: Gateway routes OpenAI-compatible requests by configured route rules
**Reason**: Path-prefix route rules are replaced by model-catalog resolution; the OpenAI-family endpoint paths are fixed and the body `model` selects the deployment chain.
**Migration**: See `model-catalog` — requests resolve by catalog model; `GET /v1/models` is gateway-owned.

### Requirement: Gateway routes Anthropic-compatible requests by configured route rules
**Reason**: Path-prefix route rules are replaced by model-catalog resolution; the Anthropic-family endpoint paths are fixed and the body `model` selects the deployment chain.
**Migration**: See `model-catalog`.

### Requirement: Gateway validates failover chain consistency at startup
**Reason**: There are no configured route chains to validate at startup. Deployment references are validated at catalog mutation time, and protocol homogeneity is enforced per-request by the protocol filter (mixed-protocol deployments under one model are legitimate).
**Migration**: Catalog mutations reject unknown providers and empty upstream names; enabling requires at least one deployment.

### Requirement: Gateway resolves provider-specific model aliases
**Reason**: Absorbed by the catalog: a deployment's `upstream_model` is the per-provider translation of the public model name.
**Migration**: Existing aliases are converted to deployments by the one-time migration.

### Requirement: Gateway preserves existing model pass-through when aliases are absent
**Reason**: Pass-through of un-cataloged model names ends; the catalog is the contract. A deployment whose upstream name equals the public name expresses pass-through explicitly.
**Migration**: Create catalog entries for previously pass-through models (startup migration warns per provider).

### Requirement: Gateway does not expose unresolved upstream model names through alias-enabled providers
**Reason**: Superseded: resolution starts from the catalog, so a request can only reach deployments that explicitly define an upstream name for the model.
**Migration**: None needed; the property holds by construction under catalog resolution.
