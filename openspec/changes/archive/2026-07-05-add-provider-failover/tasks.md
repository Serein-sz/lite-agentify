## 1. Config and model data structures

- [x] 1.1 Change `RouteConfig.provider: String` to `providers: Vec<String>` in `src/gateway/config.rs`
- [x] 1.2 Change `Route.provider_id: String` to `provider_ids: Vec<String>` in `src/gateway/model.rs`

## 2. Startup validation and route matching

- [x] 2.1 In `state.rs` `from_config`, reject a route with an empty `providers` chain
- [x] 2.2 In `from_config`, error when any provider id in a chain is missing, but skip the route only when all providers are missing
- [x] 2.3 In `from_config`, validate that all providers in a chain share one protocol, else `bail!`
- [x] 2.4 Change `match_route` to return the matched `&Route` (with its provider-id chain) instead of a single `(&Route, &Provider)`

## 3. Failover proxy loop

- [x] 3.1 In `router.rs` `proxy()`, iterate the matched route's provider chain, sending `body.clone()` to each candidate in order
- [x] 3.2 Classify each attempt: transport error or HTTP 5xx advances to the next provider; any other status stops and is forwarded
- [x] 3.3 Make the failover decision from upstream status before forwarding any response body, and forward the first non-failover response
- [x] 3.4 Return `502 Bad Gateway` when the entire chain is exhausted, logging each failed attempt with its provider id

## 4. Tests

- [x] 4.1 Update `tests.rs` config fixtures from `provider = "x"` to `providers = ["x"]`
- [x] 4.2 Add test: primary success skips fallback (fallback provider never contacted)
- [x] 4.3 Add test: primary transport error fails over to fallback and forwards fallback response
- [x] 4.4 Add test: primary 5xx fails over to fallback
- [x] 4.5 Add test: primary 4xx (and 429) is forwarded without contacting fallback
- [x] 4.6 Add test: exhausted chain returns gateway error
- [x] 4.7 Add test: mixed-protocol chain fails startup validation
- [x] 4.8 Add test: empty provider chain fails startup validation

## 5. Verification

- [x] 5.1 Run `cargo build` and `cargo test`, fixing any failures
