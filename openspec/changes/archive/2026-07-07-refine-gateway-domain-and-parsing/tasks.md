## 1. UsageSource 归位到 usage/

- [x] 1.1 新建 `gateway/usage/source.rs`，将 `UsageSource`（含 `Display` 实现）从 `domain/token.rs` 移入
- [x] 1.2 从 `domain/token.rs` 删除 `UsageSource`，`domain/mod.rs` 只重导出 `TokenUsage`
- [x] 1.3 `usage/mod.rs` 声明 `mod source;` 并 `pub(crate) use source::UsageSource;`，删除第 15 行 `#[cfg(test)] pub(crate) use crate::gateway::domain::UsageSource;`
- [x] 1.4 `usage/record.rs` 的 `UsageSource` import 改为 `use super::source::UsageSource;`
- [x] 1.5 `router.rs` 的 import 从 `domain::{TokenUsage, UsageSource}` 改为 `domain::TokenUsage` + 把 `UsageSource` 并入 `usage::{...}`
- [x] 1.6 `cargo build` 通过

## 2. 请求体单次解析

- [x] 2.1 在 `router.rs` 定义 `RequestPayload { json: Option<Value>, model: Option<String> }` 及 `parse(body: &Bytes) -> Self`（空 body → 全空；合法 JSON → 填 json+model；非空无效 JSON → 全空）。注:去掉了原设计的 `invalid_json` 字段——在别名分支里 `body.is_empty()` 已先判空,故 `json.is_none()` 已等价于"非空无效 JSON",单独字段会成 dead code
- [x] 2.2 `proxy()` 读到 body 后构建一次 `RequestPayload`，放入 `ProxyContext`
- [x] 2.3 `state::match_route` 签名从 `(path, body: &[u8])` 改为 `(path, model: Option<&str>)`，`proxy()` 传 `payload.model.as_deref()`
- [x] 2.4 删除 `state::extract_model`（不再有调用者）
- [x] 2.5 `body_for_provider` 改为接收 `&RequestPayload`（+ 原始 `&Bytes` 供透传克隆）：无别名/空 body → 透传并用 `payload.model`；配别名+非空 → `json.is_none()`(即无效 JSON) 时 `bail!`，`Some(value)` 时 `value.clone()` 后改写 `model`，不再 `from_slice`
- [x] 2.6 删除 `router::request_model`（被 `RequestPayload` 取代）
- [x] 2.7 `cargo build` 通过，确认全文件已无重复 `serde_json::from_slice` 于请求路径

## 3. 验证

- [x] 3.1 运行 `cargo test`，全部单元与集成测试通过，测试总数不减少
- [x] 3.2 补充/确认覆盖"别名 provider + 非空无效 JSON → BAD_REQUEST"分支的断言
- [x] 3.3 运行 `cargo clippy --all-targets` 与 `cargo fmt --check`，修复告警与格式
- [x] 3.4 复核 `git diff`：确认无对外 HTTP 行为、TOML 配置、`usage_records` 表结构、依赖项变更
