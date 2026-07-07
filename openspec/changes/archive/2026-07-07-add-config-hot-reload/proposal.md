# Proposal: add-config-hot-reload

## Why

网关目前只在进程启动时读取一次 TOML 配置，之后 `GatewayState` 完全不可变。任何 provider、路由、模型别名、定价或网关密钥的变更都必须重启进程，导致在途请求中断、服务出现可用性缺口。随着 provider 与别名配置的调整越来越频繁（近期连续多个 change 都在扩展配置面），需要在不重启、不影响在途请求的前提下让配置变更生效。

## What Changes

- 引入配置快照热交换机制：请求处理逻辑改为在每个请求开头读取当前配置快照（`Arc<ArcSwap<GatewayState>>`），在途请求继续持有旧快照直到处理完成，实现优雅切换。
- 新增统一的 `reload()` 入口：重新读取配置文件 → 解析 TOML → 复用现有 `from_config` 校验构建新快照 → 原子替换；任一步失败则保留旧快照继续服务并记录错误日志，绝不半更新。
- 新增文件监听触发：通过 `notify` crate 监听配置文件所在目录（跨平台），带防抖处理编辑器多次写入与原子替换写（临时文件改名）。
- 新增管理端点触发：`POST /reload`，复用现有 gateway_key 鉴权，响应体返回加载成功或失败的具体原因。
- 可热加载字段：`providers`（含 `model_aliases`）、`routes`、`pricing`、`gateway_keys`。
- 不可热加载字段：`listen_addr`（socket 已绑定）与 `usage_database`（连接池生命周期）；reload 时检测到这两者变化则记录 warn 日志提示需重启生效，其余字段照常生效。

## Capabilities

### New Capabilities

- `config-hot-reload`: 网关配置热加载——快照原子交换、文件监听与管理端点两种触发方式、失败保留旧配置的安全语义、不可热加载字段的警告行为。

### Modified Capabilities

<!-- llm-gateway 现有请求路由/鉴权/计费等需求本身不变；配置生效时机属于新能力，不修改既有 requirement。 -->

## Impact

- **代码**：
  - `src/gateway/state.rs`：`GatewayState` 保持不变，新增共享快照包装（`Arc<ArcSwap<GatewayState>>`）。
  - `src/gateway/router.rs`：axum state 换为共享快照，`proxy`/`healthz` 入口处 `load()` 一次；新增 `/reload` 路由。
  - `src/gateway/config.rs`：记录配置文件路径供 reload 复用；提取不可热加载字段的变更检测。
  - `src/main.rs`：启动文件监听后台任务。
  - 新模块（如 `src/gateway/reload.rs`）：`reload()` 核心逻辑与文件监听。
- **依赖**：新增 `arc-swap`（无锁快照读）、`notify`（跨平台文件监听）。
- **行为**：请求处理路径逻辑零改动，仅入口处取快照；`gateway_keys` 热更新意味着密钥轮换立即生效（旧 key 立刻 401，属预期行为）。
- **不影响**：usage 记录、pricing 计算、failover 逻辑均无行为变化。
