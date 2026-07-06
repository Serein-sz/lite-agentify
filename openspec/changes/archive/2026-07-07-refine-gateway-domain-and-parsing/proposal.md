## Why

上一次 `restructure-gateway-modules` 重构后残留两处内部瑕疵：`UsageSource` 只被 usage 相关代码使用却被放进了共享的 `domain/`（pricing 从不引用它，`usage/mod.rs` 甚至要 `#[cfg(test)]` 从 domain 把它重新导出回来），使 domain 边界名不副实；同一请求体在 `match_route` 与 `body_for_provider` 中被重复解析 2~3 次 JSON，既不优雅也做了无谓的重复工作。

## What Changes

- 将 `UsageSource` 从 `gateway/domain/` 移回 `gateway/usage/`，使 `domain/` 只保留真正跨 pricing/usage 两域的 `TokenUsage`。
- 移除 `usage/mod.rs` 中 `#[cfg(test)]` 从 domain 重新导出 `UsageSource` 的补丁，`usage` 直接拥有该类型。
- 消除请求体重复 JSON 解析：请求路径上只解析一次请求体，把已解析出的 `model`（或解析后的值）向下传递给别名解析，避免 `match_route` 与 `body_for_provider`/`request_model` 各自从零 `serde_json::from_slice`。
- 无对外行为变更，非破坏性重构。

## Capabilities

### New Capabilities

（无。本次为纯内部重构，不引入新的对外能力。）

### Modified Capabilities

（无。`llm-gateway` 规格描述的全部是行为需求，本次仅调整内部类型归属与请求体解析路径，不改变任何需求级行为。）

## Impact

- **受影响代码**：`src/gateway/domain/`（移出 `UsageSource`）、`src/gateway/usage/`（接收 `UsageSource`、更新重导出）、`src/gateway/router.rs`（import 路径 + 解析路径去重）、`src/gateway/state.rs`（`match_route` 签名可能调整以复用已解析的 model）。
- **无变更**：对外 HTTP API、TOML 配置结构、PostgreSQL `usage_records` 表结构、依赖项均不变。
- **验证方式**：现有单元测试与集成测试（`gateway/tests.rs`）全部保持通过，作为无行为回归的保证。
