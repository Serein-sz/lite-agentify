## Context

`restructure-gateway-modules` 完成后，`gateway/` 内部残留两处瑕疵：

1. **domain 边界名不副实**：`domain/` 本意是存放 pricing 与 usage 都要用的跨域类型，但实际只有 `TokenUsage` 满足这一点。`UsageSource`（`ProviderResponse`/`StreamSummary`/`Unavailable`）只被 usage 相关代码引用，pricing 从不碰它。反证是 `usage/mod.rs` 里那行 `#[cfg(test)] pub(crate) use crate::gateway::domain::UsageSource;`——usage 必须把一个"本该属于自己"的类型从 domain 重新导出回来。

2. **请求体重复 JSON 解析**：同一份请求体在一次请求中被 `serde_json::from_slice` 解析多次：
   - `state.match_route(path, body)` → `extract_model(body)` 解析一次取 `model`；
   - `body_for_provider(body, provider)` → `request_model(body)` 每尝试一个 provider 就解析一次取 `model`；
   - 若该 provider 配了别名，`body_for_provider` 再 `from_slice` 一次以改写 `model`。
   
   在 N 个 provider 的失败转移链上，总解析次数是 `1 + N + (配别名的 provider 数)`。

现有约束（必须精确保留的行为边界）：

- `extract_model` 与 `request_model` 对空 body 与无效 JSON 都静默返回 `None`。
- `body_for_provider` 在 **provider 配了别名且 body 非空** 时会 `serde_json::from_slice(...).context(...)?`，即**非空但无效 JSON 会返回 `Err` → 上层转成 `BAD_REQUEST`**。这是一个真实的行为分支（无别名的 provider 则直接透传无效 body），去重后必须一模一样地保留。

## Goals / Non-Goals

**Goals:**

- 把 `UsageSource` 从 `domain/` 移回 `usage/`，`domain/` 只保留真正跨域的 `TokenUsage`。
- 删除 `usage/mod.rs` 中从 domain 重导出 `UsageSource` 的 `#[cfg(test)]` 补丁。
- 请求体在一次请求中只解析一次 JSON，解析结果向下复用于路由匹配与所有 provider 的别名解析。
- 全程零行为变更；现有单元测试与集成测试保持通过作为回归护栏。

**Non-Goals:**

- 不重构 `record_usage` 的 `source` 参数（"半失效"表达问题留待后续，本次不动，控制风险与范围）。
- 不改变 `UsageMetadata`/`UsageRecord` 的字段布局。
- 不改变任何对外 HTTP 行为、TOML 配置、`usage_records` 表结构或依赖项。

## Decisions

### Decision 1：`UsageSource` 从 `domain/` 移回 `usage/`

将 `UsageSource`（含 `Display` 实现）移入 `usage/`，`domain/token.rs` 只留 `TokenUsage`。

- **落点**：新建 `usage/source.rs` 存放 `UsageSource`，由 `usage/mod.rs` 重导出；或直接并入已有的 `usage/record.rs`（`UsageRecord` 是它唯一的结构体依赖）。倾向**独立 `usage/source.rs`**，与其它单一职责文件保持一致风格。
- **连带清理**：删除 `usage/mod.rs` 第 15 行 `#[cfg(test)] pub(crate) use crate::gateway::domain::UsageSource;`。`router.rs` 的 import 从 `domain::{TokenUsage, UsageSource}` 改为 `domain::TokenUsage` + `usage::{... , UsageSource}`。`usage/record.rs` 的 `use crate::gateway::domain::UsageSource;` 改为 `use super::source::UsageSource;`（或同模块引用）。
- **测试影响**：`tests.rs:826` 用的是 `super::usage::UsageSource::Unavailable`，路径不变（仍从 `usage` 出），无需改动。
- **Why**：让 `domain/` 名副其实——只装真正被多个域共享的类型。`UsageSource` 是"用量记录来源"，纯属 usage 领域。

### Decision 2：请求体单次解析，`RequestPayload` 向下传递

在 `proxy()` 读到 body 后，一次性解析为一个轻量载体，替代散落的多次 `from_slice`：

```rust
/// 请求体只解析一次的结果，供路由匹配与各 provider 的别名解析复用。
struct RequestPayload {
    /// body 非空且为合法 JSON 时的解析值。
    json: Option<Value>,
    /// 顶层字符串 `model`（若存在）。
    model: Option<String>,
    /// body 非空但不是合法 JSON 时为 true（用于精确保留别名路径的错误语义）。
    invalid_json: bool,
}

impl RequestPayload {
    fn parse(body: &Bytes) -> Self {
        if body.is_empty() {
            return Self { json: None, model: None, invalid_json: false };
        }
        match serde_json::from_slice::<Value>(body) {
            Ok(value) => {
                let model = value.get("model").and_then(Value::as_str).map(str::to_owned);
                Self { json: Some(value), model, invalid_json: false }
            }
            Err(_) => Self { json: None, model: None, invalid_json: true },
        }
    }
}
```

- `match_route` 签名从 `(&self, path: &str, body: &[u8])` 改为 `(&self, path: &str, model: Option<&str>)`；`proxy()` 传 `payload.model.as_deref()`。`state::extract_model` 随之删除（不再有调用者）。
- `body_for_provider` 改为接收 `&RequestPayload`（+ 原始 `&Bytes` 用于透传克隆），逻辑改写为：
  - `provider.model_aliases.is_empty() || body.is_empty()` → 透传（`requested_model` 取 `payload.model`）；
  - 否则（配别名 + 非空）：若 `payload.json` 为 `None`（即 `invalid_json`）→ `bail!`（保留原 `?` 的 `Err`→`BAD_REQUEST`）；若为 `Some(value)` → `value.clone()` 后改写 `model`，**复用已解析值，不再 `from_slice`**。
- **解析次数**：由 `1 + N + 别名provider数` 降为**恒定 1 次**。别名改写用 `Value::clone()`（比重解析字节更省）。

### Decision 3：精确保留无效-JSON 的错误分支

去重后，"provider 配别名 + body 非空但无效 JSON → `BAD_REQUEST`"这一分支通过 `invalid_json`/`json.is_none()` 判定后 `bail!` 保留，与改写前 `from_slice(...).context(...)?` 的可观测结果完全一致（都是 `Err` → `ProviderAttempt::Forward(BAD_REQUEST)`）。

- **Why**：本次是零行为变更重构，即便这个边界本身可议（无别名 provider 反而透传无效 body），也不在本次修正范围，原样保留。

## Risks / Trade-offs

- **[match_route 签名变更波及测试]** → `matches_model_prefix_route` 等若直接调用 `state.match_route(path, body)` 需改为传 `model`。缓解：改动后 `cargo test` 全绿；若测试走的是 router 端到端则不受影响。
- **[无效-JSON 边界回归]** → 去重最容易在此处引入偏差。缓解：保留/新增针对"别名 provider + 无效 JSON → BAD_REQUEST"的测试断言，确保 `bail!` 分支被覆盖。
- **[Value::clone 开销]** → 别名改写从"重解析"变为"克隆 Value"。权衡：克隆已解析的 `Value` 通常比从字节重新解析更快，且每请求最多克隆一次（仅命中别名时），净收益为正。
- **[范围克制]** → 未一并修 `record_usage` 的 source 参数，可能显得只做了一半。权衡：那是独立主观改动，单独成 change 更利于审查与回滚。
