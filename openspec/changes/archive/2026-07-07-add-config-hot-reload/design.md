# Design: add-config-hot-reload

## Context

网关配置目前的生命周期是"启动时一次性加载"：`main.rs` 调 `load_config_from_env()` 解析 TOML，`build_router()` 里 `GatewayState::from_config_with_upstream_and_recorder()` 完成全部校验并构建不可变状态，随后通过 axum `with_state` 分发。`GatewayState` 的所有配置字段（`gateway_keys`、`providers`、`routes`、`pricing`）都是 `Arc` 包裹的只读数据，请求处理逻辑从不修改它们——这为快照交换提供了理想前提：不需要改动任何请求处理逻辑，只需让请求拿到"最新的快照"。

运行平台包含 Windows，因此触发机制必须跨平台（排除 SIGHUP）。

## Goals / Non-Goals

**Goals:**

- 配置文件变更后无需重启即可生效（providers / model_aliases / routes / pricing / gateway_keys）
- 在途请求不受 reload 影响，持有旧快照直到自然完成
- 新配置解析或校验失败时保留旧配置继续服务，绝不出现半更新状态
- 文件监听与管理端点两种触发方式，全部跨平台
- reload 结果可观测（日志 + 端点响应体）

**Non-Goals:**

- `listen_addr` 热加载（socket 已绑定，需重启）
- `usage_database` 热加载（连接池与 recorder 生命周期复杂，第一版不做）
- 配置格式变更或新增配置字段
- 多配置文件 / include 机制

## Decisions

### D1: 快照交换用 `arc-swap` 而非 `RwLock<Arc<...>>` 或重建 Router

- **选择**：axum state 类型改为包含 `Arc<ArcSwap<GatewaySnapshot>>` 的轻量包装；`proxy` 与 `/reload` 处理器在入口处 `load_full()` 取快照，之后整个请求生命周期使用该快照。
- **理由**：读路径无锁、无等待，符合"读极多写极少"的模式；请求持有 `Arc` 快照天然实现优雅切换。`RwLock` 在写时会阻塞读（或读阻塞写），且容易把锁跨 await 持有；重建整个 Router 则需要更换 listener 服务方式，改动面大得多。
- **备选**：`tokio::sync::watch`（多一层订阅语义，这里不需要变更通知）；每请求重读配置文件（IO 开销与解析失败语义不可接受）。

### D2: 交换粒度为整个状态快照，而非逐字段交换

- **选择**：`reload()` 用现有 `from_config_*` 构建一个全新的完整状态（keys、providers、routes、pricing 一起），一次原子替换。
- **理由**：现有校验逻辑（provider 去重、路由协议一致性、别名非空等）是针对整体配置的，整体构建 = 免费的 reload 前预检；逐字段交换会引入字段间不一致窗口（如新 route 引用旧 providers 里不存在的 id）。
- **注意**：`upstream` 客户端与 `usage_recorder` 不随 reload 重建，从旧快照沿用（见 D4）。

### D3: 双触发共享同一 `reload()` 入口

- **选择**：`notify` 文件监听后台任务与 `POST /reload` 端点都调用同一个 `reload(shared, config_path)` 函数。
- **文件监听细节**：监听配置文件**所在目录**而非文件本身（编辑器原子写为"写临时文件后 rename"，监听文件会丢失 inode/handle）；事件按文件名过滤；防抖约 500ms 合并编辑器多次写事件；监听任务失败只记日志，不影响主服务。
- **端点细节**：`POST /reload` 复用现有 gateway_key 鉴权（`is_authorized`）；成功返回 200 与简要摘要，失败返回 500 与错误原因文本（不泄露密钥等敏感值）；注册为显式路由，优先于 fallback proxy。
- **备选**：只做其中一种——文件监听失败不可见、端点需要手动触发，两者互补且共享核心后增量成本极小。

### D4: 不可热加载字段的处理

- **选择**：reload 时对比新旧配置的 `listen_addr` 与 `usage_database`，发现变化记录 `warn!("... requires restart")`，忽略这两个字段（沿用旧 listener 与旧 recorder），其余字段照常生效。
- **理由**：静默忽略会让用户误以为生效；直接失败会阻止其余合法变更生效。警告 + 部分生效是最实用的折中。
- **`usage_database` 对比方式**：对比配置结构体值（url / enabled / max_connections），不重建连接池。

### D5: reload 失败语义

- **选择**：读文件、解析 TOML、`from_config` 校验任一步失败 → 不触碰当前快照，`error!` 日志记录原因；端点触发时把错误信息（`anyhow` 链）返回给调用方。
- **理由**：网关的首要职责是持续服务；坏配置只应影响"想让它生效的人"的反馈通道，不应影响流量。

### D6: 模块划分

- 新增 `src/gateway/reload.rs`：`SharedGatewayState`（ArcSwap 包装 + `load()` / `store()`）、`reload()` 函数、`spawn_config_watcher()` 文件监听任务。
- `config.rs` 增加 `resolve_config_path()`（现有 env/默认路径逻辑提取，reload 复用同一路径）。
- `router.rs`：`build_router` 返回共享状态供 `main.rs` 启动 watcher；`with_state(SharedGatewayState)`。

## Risks / Trade-offs

- [编辑器写入行为差异（多次写、原子 rename、截断后写）导致监听丢事件或读到半个文件] → 监听目录 + 防抖合并；即使读到半个文件，TOML 解析失败会走 D5 保留旧配置，下一次事件重试；端点作为兜底手动触发。
- [`gateway_keys` 热更新使旧 key 立即 401] → 属预期行为（密钥轮换即时生效）；文档中明示；轮换时可先加新 key reload、客户端切换后再删旧 key reload，实现无缝轮换。
- [reload 与在途请求并发] → 请求入口 `load_full()` 后持有 `Arc`，旧快照引用计数归零自动释放，无需显式排空。
- [`/reload` 端点被滥用触发频繁 reload] → 已有 gateway_key 鉴权；reload 本身幂等且开销小（读文件 + 解析），暂不做限流。
- [notify 后台任务 panic 或 watcher 失效后热加载静默失效] → 任务内错误全部捕获记日志；watcher 创建失败在启动日志中 warn；端点触发不依赖 watcher。
- [快照中 `upstream` / `usage_recorder` 沿用旧实例] → 有意为之（D4）；风险是 usage_database 配置变化不生效，已用 warn 日志覆盖。

## Migration Plan

纯增量变更，无破坏性：不改配置格式、不改现有端点行为。部署即生效；回滚 = 回退二进制。`/reload` 是新端点，旧客户端不受影响。

## Open Questions

- 防抖窗口具体值（300–1000ms 均可，实现时取 500ms 起步，必要时可配置化——第一版硬编码）。
- `/reload` 响应体格式：第一版纯文本足够，后续如需程序化消费可改 JSON（非阻塞）。
