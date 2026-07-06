## 1. 共享领域层 domain/

- [x] 1.1 创建 `gateway/domain/mod.rs` 与 `gateway/domain/token.rs`，将 `TokenUsage`（含 `has_tokens`/`merge_from`）与 `UsageSource`（含 `Display`）从 `usage.rs` 迁入 `token.rs`
- [x] 1.2 在 `gateway/mod.rs` 声明 `mod domain;` 并按需重导出，`cargo build` 通过

## 2. 定价模块 pricing/

- [x] 2.1 创建 `gateway/pricing/mod.rs`，声明 `model`/`calc`/`config` 子模块并重导出对外符号
- [x] 2.2 迁移 `Pricing`、`PricingMap` 到 `pricing/model.rs`
- [x] 2.3 迁移 `calculate_cost`、`lookup_pricing`、`token_cost` 及 `PRICING_WILDCARD`/`TOKENS_PER_MILLION` 到 `pricing/calc.rs`，依赖 `domain::TokenUsage`
- [x] 2.4 迁移 `pricing_map()`、`validate_non_negative`、`validate_optional_non_negative` 到 `pricing/config.rs`，继续读取 `config::PricingConfig`
- [x] 2.5 将 pricing 相关内联单测（`calculates_cache_aware_cost`、`openai_cached_tokens_are_subtracted_from_regular_input`、`missing_cache_pricing_leaves_cost_unavailable`、`anthropic_cache_read_exceeding_input_stays_non_negative`、`pricing_lookup_falls_back_by_specificity`）下沉到 `pricing/calc.rs`
- [x] 2.6 在 `gateway/mod.rs` 声明 `mod pricing;`，`cargo build` 通过

## 3. 用量模块 usage/

- [x] 3.1 创建 `gateway/usage/mod.rs`，声明 `record`/`parse`/`observer`/`recorder`/`entity` 子模块并重导出对外符号
- [x] 3.2 迁移 `UsageRecord` 到 `usage/record.rs`
- [x] 3.3 迁移 `parse_non_streaming_usage`、`parse_openai_usage`、`parse_anthropic_usage`、`number` 到 `usage/parse.rs`
- [x] 3.4 迁移 `UsageObserver`（`new`/`feed`/`finish`/`consume_line`）到 `usage/observer.rs`
- [x] 3.5 迁移 `UsageRecorder` trait、`UsageRecordFuture`、`NoopUsageRecorder`、`MemoryUsageRecorder`、`SeaOrmUsageRecorder`、`recorder_from_config`、`warn_record_error` 到 `usage/recorder.rs`
- [x] 3.6 迁移 `mod usage_record` ORM 实体到 `usage/entity.rs`
- [x] 3.7 将 parse/observer 相关内联单测（`parses_openai_cached_usage`、`parses_anthropic_cache_usage`、`observer_merges_usage_fields_across_events`、`observer_reassembles_usage_line_split_across_chunks`、`observer_parses_final_line_without_trailing_newline`、`observer_without_usage_returns_none`）分别下沉到 `usage/parse.rs` 与 `usage/observer.rs`
- [x] 3.8 删除原 `usage.rs`，在 `gateway/mod.rs` 将 `mod usage;` 指向新目录，`cargo build` 通过

## 4. 更新引用方

- [x] 4.1 更新 `state.rs` 的 import 路径（`PricingMap`/`pricing_map` 来自 `pricing`，`UsageRecorder`/`NoopUsageRecorder` 来自 `usage`）
- [x] 4.2 更新 `router.rs` 的 import 路径（`calculate_cost` 来自 `pricing`，`UsageRecord`/`UsageObserver`/`UsageSource`/`parse_non_streaming_usage`/`recorder_from_config`/`warn_record_error` 来自 `usage`/`domain`）
- [x] 4.3 更新 `gateway/mod.rs` 的模块声明与 `pub use` 重导出，`cargo build` 通过

## 5. 重构 router::proxy()

- [x] 5.1 在 `router.rs` 定义 `ProviderAttempt` 三态枚举（`Forward(Response)`/`Failover(Response)`/`AliasMissing`）
- [x] 5.2 抽取单 provider 尝试为辅助函数（别名解析、构建 URI、构建 headers、发送、按状态判定），返回 `ProviderAttempt`
- [x] 5.3 将 `proxy()` 循环体改为对 `ProviderAttempt` 的 match 分发，保留原有 `last_error`/`unresolved_model_alias` 的最终错误语义
- [x] 5.4 `cargo build` 通过，`proxy()` 循环体显著收缩

## 6. 验证

- [x] 6.1 运行 `cargo test`，确认全部单元测试与 `gateway/tests.rs` 集成测试通过，测试总数不减少
- [x] 6.2 运行 `cargo clippy --all-targets` 与 `cargo fmt --check`，修复告警与格式
- [x] 6.3 复核 `git diff`：确认无对外 HTTP 行为、TOML 配置、`usage_records` 表结构、依赖项变更
