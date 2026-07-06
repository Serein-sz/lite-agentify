## MODIFIED Requirements

### Requirement: Gateway observes streaming usage without rewriting streams
The system SHALL observe provider-native streaming responses for usage metadata while forwarding stream bytes to clients unchanged. The system SHALL parse SSE events incrementally as bytes arrive rather than buffering the full response body, and SHALL merge usage fields reported across separate stream events into a single usage result.

#### Scenario: OpenAI-compatible stream exposes final usage
- **WHEN** an OpenAI-compatible streaming response includes final usage metadata
- **THEN** the gateway MUST persist token usage and estimated cost after the stream completes without rewriting SSE event payloads.

#### Scenario: Anthropic-compatible stream reports input and output tokens across events
- **WHEN** an Anthropic-compatible streaming response reports input tokens in a `message_start` event and output tokens in a `message_delta` event
- **THEN** the gateway MUST persist both the input tokens from the `message_start` usage and the output tokens from the `message_delta` usage after the stream completes without rewriting SSE event payloads.

#### Scenario: Usage fields split across events are merged
- **WHEN** a streaming response reports different token classes in separate SSE events
- **THEN** the gateway MUST merge the reported token classes into a single usage result rather than replacing earlier fields with only the latest event.

#### Scenario: Usage payload spans chunk boundaries
- **WHEN** a single SSE usage line is delivered across more than one stream chunk
- **THEN** the gateway MUST reassemble the line before parsing and MUST persist the usage it reports.

#### Scenario: Streaming usage cannot be parsed
- **WHEN** a streaming response is missing parseable usage metadata or usage parsing fails
- **THEN** the gateway MUST preserve the client stream and persist request metadata with usage source unavailable.
