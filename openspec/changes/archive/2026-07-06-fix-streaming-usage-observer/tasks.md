## 1. Incremental SSE Parsing

- [x] 1.1 Replace the full-response `buffer` in `UsageObserver` with a partial-line buffer that retains only bytes not yet terminated by a newline.
- [x] 1.2 On each `feed`, split out completed lines, consume them immediately, and keep only the trailing unterminated fragment.
- [x] 1.3 Process any remaining unterminated line in `finish` so a final line without a trailing newline is not dropped.
- [x] 1.4 Validate UTF-8 per line rather than over the whole response buffer.

## 2. Cross-Event Usage Merge

- [x] 2.1 Add a field-level merge for `TokenUsage` that updates a token class only when a later event reports it and preserves earlier fields otherwise.
- [x] 2.2 Apply the merge as SSE usage events are consumed so the final usage reflects all events instead of only the last.
- [x] 2.3 Preserve existing behavior for OpenAI streams, where a single final event carries the complete usage object.

## 3. Anthropic Streaming Path Fix

- [x] 3.1 Read `message_start` usage from `/message/usage` to capture `input_tokens` and cache creation/read tokens.
- [x] 3.2 Read `message_delta` usage from `/usage` (sibling of `delta`) to capture `output_tokens`.
- [x] 3.3 Remove the non-existent `/delta/usage` path.
- [x] 3.4 Keep parsing tolerant so events without parseable usage are skipped and a stream with no parseable usage records `usage_source = unavailable`.

## 4. Tests and Verification

- [x] 4.1 Add a test that an Anthropic stream with `message_start` and `message_delta` records both `input_tokens` and `output_tokens`.
- [x] 4.2 Add a test that usage fields split across separate events are merged rather than overwritten.
- [x] 4.3 Add a test that a usage payload split across chunk boundaries (partial line) is parsed correctly.
- [x] 4.4 Keep the existing OpenAI streaming usage test passing as a regression guard.
- [x] 4.5 Add a test that a stream with no parseable usage still forwards bytes unchanged and records usage source unavailable.
- [x] 4.6 Run `cargo test` and verify OpenSpec status before implementation is considered complete.
