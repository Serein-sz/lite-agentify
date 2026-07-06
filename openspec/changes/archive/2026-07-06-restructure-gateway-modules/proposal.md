## Why

`gateway/usage.rs`（约 600 行）把定价、用量类型、协议解析、流式观察、持久化五个独立领域混在一个文件里，`router.rs` 的 `proxy()` 单个函数约 200 行且嵌套很深，难以阅读和维护。同时定价（pricing）后续需要开放"定价管理"功能，必须先从 usage 中独立为平级能力，才能承载自己的持久化与管理逻辑而不污染用量记录。

## What Changes

- 将 `usage.rs` 拆分为 `usage/` 子模块：`record`（UsageRecord）、`parse`（协议解析）、`observer`（流式 UsageObserver）、`recorder`（Recorder trait + Noop/Memory/SeaOrm 实现）、`entity`（usage_record ORM）。
- 将定价提升为平级 `pricing/` 模块：`model`（Pricing/PricingMap）、`calc`（calculate_cost/lookup/token_cost）、`config`（pricing_map 构建 + 校验），为未来定价管理功能预留结构。
- 将 `TokenUsage` / `UsageSource` 提为共享领域类型，放入 `domain/`，使 pricing 与 usage 都能依赖而互不耦合。
- 重构 `router::proxy()`：将"单个 provider 尝试"抽为返回三态枚举（成功/重试/别名缺失）的辅助函数，收缩循环体。
- 各模块内联单元测试随源码下沉到对应子模块；集成测试 `tests.rs` 本次不拆分。
- 无对外行为变更，非破坏性重构。

## Capabilities

### New Capabilities

（无。本次为纯内部模块重构，不引入新的对外能力。）

### Modified Capabilities

（无。`llm-gateway` 规格描述的全部是行为需求，本次仅调整内部模块结构，不改变任何需求级行为。）

## Impact

- **受影响代码**：`src/gateway/` 内部模块结构（`usage.rs`、`router.rs`、`state.rs` 的 import 路径、`mod.rs` 的模块声明与重导出）。
- **无变更**：对外 HTTP API、TOML 配置结构、PostgreSQL `usage_records` 表结构、依赖项（sea-orm/rust_decimal 等）均不变。
- **验证方式**：现有单元测试与集成测试（`gateway/tests.rs`）全部保持通过，作为无行为回归的保证。
