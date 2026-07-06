## Why

The streaming usage observer buffers the entire SSE response body in memory just to extract a small usage summary that appears near the end of the stream, so retained memory grows with response length across every concurrent stream. The same observer also parses Anthropic streaming usage from the wrong JSON path and overwrites usage instead of merging it across events, so Anthropic streaming requests silently lose `input_tokens` and undercount input cost. No test covers Anthropic streaming usage, so the defect is invisible.

## What Changes

- Replace the full-stream buffer with incremental line-oriented SSE parsing that retains only an unterminated partial line plus the latest observed usage, making retained memory independent of response length.
- Fix Anthropic streaming usage extraction to read `input_tokens` from the `message_start` event (`/message/usage`) and `output_tokens` from the `message_delta` event (`/usage`), removing the non-existent `/delta/usage` path.
- Change streaming usage aggregation from whole-record replacement to field-level merge, so token classes reported across separate SSE events (Anthropic `message_start` input, `message_delta` output) are combined rather than overwritten.
- Add tests for Anthropic streaming usage, cross-event merge, and chunk boundaries that split SSE lines.

## Capabilities

### New Capabilities

### Modified Capabilities
- `llm-gateway`: Refine streaming usage observation to parse incrementally and to correctly merge provider-native usage across streaming events without rewriting client stream bytes.

## Impact

- Affected code: `src/gateway/usage.rs` (`UsageObserver`, `TokenUsage`, Anthropic stream parsing), `src/gateway/tests.rs` streaming usage tests.
- Behavior: streaming responses continue to be forwarded byte-for-byte unchanged; only the observer's parsing and memory profile change.
- Data: Anthropic streaming usage records will now include `input_tokens` and, when priced, more accurate estimated input cost.
- Non-goals: no change to non-streaming parsing, cost formula, persistence schema, or the proxy/failover path.
