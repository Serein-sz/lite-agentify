## 1. Fix Cost Calculation

- [x] 1.1 In `calculate_cost`, stop subtracting `cache_read_tokens` and `cache_write_tokens` from regular input tokens; subtract only `cached_input_tokens` (the OpenAI cached-prompt subset).
- [x] 1.2 Clamp `regular_input` to a non-negative value so estimated cost can never be negative.
- [x] 1.3 Add a brief comment explaining why only `cached_input_tokens` is subtracted (it is the only class reported as a subset of input tokens).

## 2. Tests and Verification

- [x] 2.1 Update `calculates_cache_aware_cost` expected value to reflect that Anthropic cache read/write tokens are billed in addition to full input tokens.
- [x] 2.2 Add a test that Anthropic usage with `cache_read_tokens` greater than `input_tokens` produces a non-negative cost.
- [x] 2.3 Add a test that OpenAI cached-prompt subtraction still applies (regular input excludes cached tokens).
- [x] 2.4 Run `cargo test`, `cargo clippy`, and `cargo fmt --check`; verify OpenSpec status before implementation is considered complete.

