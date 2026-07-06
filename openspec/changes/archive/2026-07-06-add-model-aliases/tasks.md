## 1. Configuration And Runtime Model

- [x] 1.1 Add optional `model_aliases` parsing to provider configuration.
- [x] 1.2 Carry provider alias mappings into the runtime provider model.
- [x] 1.3 Validate alias mappings at startup, including non-empty public and upstream model names.

## 2. Alias Resolution

- [x] 2.1 Add request body rewriting for the top-level JSON string `model` field.
- [x] 2.2 Resolve aliases separately for each provider attempt inside the failover loop.
- [x] 2.3 Skip alias-enabled providers that cannot resolve the requested public model.
- [x] 2.4 Return a gateway error when no provider in the matched chain can resolve the requested public model.
- [x] 2.5 Preserve pass-through request bodies for providers without aliases.

## 3. Tests

- [x] 3.1 Add a test that rewrites a public model alias to the selected provider's upstream model.
- [x] 3.2 Add a test that fallback providers receive their own provider-specific upstream model mapping.
- [x] 3.3 Add a test that alias-enabled providers are not contacted when they cannot resolve the requested model.
- [x] 3.4 Add a test that providers without aliases preserve existing model pass-through behavior.
- [x] 3.5 Add a test that non-model request fields and provider-native responses are not rewritten.

## 4. Verification

- [x] 4.1 Run formatting checks.
- [x] 4.2 Run the Rust test suite.
