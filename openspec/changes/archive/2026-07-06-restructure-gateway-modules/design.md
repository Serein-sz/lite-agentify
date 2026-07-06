## Context

`src/gateway/` 目前是扁平模块结构。随着失败转移、模型别名、Token 用量与成本记录相继落地，两个文件积累了过多职责：

- `usage.rs`（约 600 行）混合了五个独立领域：定价（Pricing/成本计算/校验）、用量类型（TokenUsage/UsageSource/UsageRecord）、协议解析（OpenAI/Anthropic usage 解析）、流式观察（UsageObserver）、持久化（Recorder trait + Noop/Memory/SeaOrm 实现 + ORM 实体）。
- `router.rs` 的 `proxy()` 单个 async 函数约 200 行，内含鉴权、读 body、路由匹配、以及一个深度嵌套的 provider 失败转移循环。

现有约束：

- `calculate_cost(pricing, usage)` 同时依赖定价与用量两个域，提升 pricing 为平级后必须选定归属。
- `PricingMap = Arc<HashMap<(String,String), Pricing>>` 当前是不可变结构，启动时一次性构建。
- 所有对外行为已被 `openspec/specs/llm-gateway/spec.md` 与 `gateway/tests.rs`（40+ 集成测试）覆盖。

## Goals / Non-Goals

**Goals:**

- 按领域拆分 `usage.rs`，每个文件单一职责。
- 将 pricing 提升为平级 `pricing/` 模块，为后续"定价管理"功能预留结构。
- 收缩 `router::proxy()` 的循环体，提升可读性。
- 全程零行为变更；现有单元测试与集成测试保持通过作为回归护栏。

**Non-Goals:**

- 不改变任何对外 HTTP 行为、TOML 配置结构、`usage_records` 表结构或依赖项。
- 不实现"定价管理"功能本身（本次仅预留结构）。
- 不拆分集成测试 `tests.rs`（边界未稳定，本次不动）。
- 不将 pricing 改造为运行时可变结构（见 Decision 2）。

## Decisions

### Decision 1：`TokenUsage` / `UsageSource` 归入共享 `domain/`

`calculate_cost` 同时碰 pricing 与 usage。将 `TokenUsage`、`UsageSource` 提为共享领域类型放入 `gateway/domain/`，`pricing` 与 `usage` 都依赖它而互不依赖。

- **Why**：`TokenUsage` 是稳定的小数据结构（一组 token 计数），不属于任一域私有。共享后 pricing 可独立演化，无需反向依赖 usage。
- **Alternative（方案 B，未采纳）**：`TokenUsage` 留在 usage，pricing 反向依赖 usage。被否，因为 pricing 后续会长出持久化/CRUD/handler，让它依赖 usage 会把"会独立演化的能力"绑死在"用量记录"上。

### Decision 2：`calculate_cost` 归属 `pricing/`,pricing 本次保持静态结构

成本计算是"定价"的核心行为,`calculate_cost`/`lookup_pricing`/`token_cost` 放入 `pricing/calc.rs`。`PricingMap` 本次维持现有的不可变 `Arc<HashMap>`。

- **Why**：本次是纯重构,不引入运行时可变性。保持静态结构可确保零行为变更,`state.rs`、`router.rs` 的调用方式不变。
- **未来注记（不在本次范围）**："定价管理"若需运行时增删改价,`PricingMap` 需换成 `Arc<RwLock<...>>` 或专门 store。`pricing/` 的目录结构为此预留了扩展位,但本次不实现。

### Decision 3：`pricing_map()` 校验逻辑随 pricing 迁移,继续依赖 `config::PricingConfig`

`pricing_map()` 及其 `validate_*` 校验函数迁入 `pricing/config.rs`,继续读取 `config::PricingConfig`。

- **Why**：校验是"从配置构建定价表"的一部分,天然属于 pricing 域。`pricing` 依赖 `config` 与现有 `usage` 依赖 `config` 方向一致,不产生循环。
- **Alternative（未采纳）**：把校验留在 config.rs。被否,校验规则（币种格式、非负价格、重复条目）是定价领域知识,不应散落在配置加载层。

### Decision 4：`router::proxy()` 抽取单 provider 尝试的三态结果

将循环体内"对单个 provider 的一次尝试"抽为辅助函数,返回三态枚举:

```rust
enum ProviderAttempt {
    Forward(Response),   // 得到可转发响应（含 2xx/3xx/4xx/429）→ 直接返回
    Failover(Response),  // 传输错误或 5xx → 记为 last_error,试下一个
    AliasMissing,        // 该 provider 未定义请求的模型别名 → 跳过
}
```

- **Why**：`proxy()` 的复杂度集中在"每个 provider 尝试后如何决定继续/返回"。抽出三态枚举后,外层循环退化为简单的 match 分发,循环体从约 150 行降到十几行。
- **Alternative（未采纳）**：拆成多个自由函数各传一堆参数。被否,三态枚举把"下一步动作"表达为类型,比布尔标志(`unresolved_model_alias` + `last_error`)组合更清晰。

### Decision 5：目标模块结构

```
gateway/
├── mod.rs          // 更新模块声明与重导出
├── config.rs       // 不动
├── headers.rs      // 不动
├── model.rs        // 不动
├── upstream.rs     // 不动
├── state.rs        // 仅更新 import 路径
├── router.rs       // proxy() 抽 ProviderAttempt,更新 import
├── domain/
│   ├── mod.rs
│   └── token.rs    // TokenUsage, UsageSource
├── pricing/
│   ├── mod.rs
│   ├── model.rs    // Pricing, PricingMap
│   ├── calc.rs     // calculate_cost, lookup_pricing, token_cost
│   └── config.rs   // pricing_map(), validate_* 校验
├── usage/
│   ├── mod.rs
│   ├── record.rs   // UsageRecord
│   ├── parse.rs    // parse_non_streaming_usage, parse_openai/anthropic, number
│   ├── observer.rs // UsageObserver
│   ├── recorder.rs // UsageRecorder trait, Noop/Memory/SeaOrm, recorder_from_config, warn_record_error
│   └── entity.rs   // usage_record ORM 实体
└── tests.rs        // 不拆
```

各模块内联 `#[cfg(test)]` 单测随其覆盖的函数下沉到对应新文件。

## Risks / Trade-offs

- **[可见性/import 面积大]** → 从单文件拆多文件会触碰大量 `pub(super)` 可见性与 import 路径。缓解:逐模块迁移,每步 `cargo build` + `cargo test` 验证,依赖现有测试套件锁定行为。
- **[单测下沉遗漏或错配]** → usage.rs 内约 300 行单测需正确分派到 parse/observer/pricing 各文件。缓解:按被测函数归属逐一迁移,迁移后测试总数不变、全绿。
- **[domain 过度设计]** → 只有两个类型进 `domain/` 可能显得薄。权衡:这是刻意的最小共享层,避免 pricing↔usage 双向依赖;若未来无更多共享类型,`domain/token.rs` 也足以自证其价值。
- **[proxy 重构引入行为偏差]** → 三态枚举必须精确复刻现有失败转移语义(仅传输错误/5xx 转移,4xx/429 直接转发)。缓解:失败转移相关集成测试(primary_success / transport_error_failover / server_error_failover / client_error_forwarded / rate_limit_forwarded / exhausted_chain)全部保持通过。
