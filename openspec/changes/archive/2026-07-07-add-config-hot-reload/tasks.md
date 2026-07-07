# Tasks: add-config-hot-reload

## 1. 基础设施与依赖

- [x] 1.1 在 `Cargo.toml` 添加 `arc-swap` 与 `notify` 依赖
- [x] 1.2 在 `config.rs` 提取 `resolve_config_path()`：把现有 env/默认路径解析逻辑抽成可复用函数，`load_config_from_env` 改为调用它；返回路径供 reload 复用

## 2. 共享快照与请求路径接入

- [x] 2.1 新建 `src/gateway/reload.rs`，定义 `SharedGatewayState`（`Arc<ArcSwap<GatewayState>>` 包装，提供 `load()` / `store()`）
- [x] 2.2 `router.rs`：axum state 改为 `SharedGatewayState`，`proxy` 入口处 `load_full()` 取快照，其余请求处理逻辑不动；`build_router` 返回 `(Router, SharedGatewayState)` 供 watcher 使用
- [x] 2.3 跑通现有测试：`tests.rs` 中构建 state 的辅助路径适配新的共享包装，所有既有测试保持通过

## 3. reload 核心逻辑

- [x] 3.1 实现 `reload(shared, config_path)`：读文件 → 解析 TOML → 用现有 `from_config` 校验构建新 `GatewayState`（沿用旧快照的 `upstream` 与 `usage_recorder`）→ 原子 `store()`；任一步失败返回错误且不触碰当前快照
- [x] 3.2 实现不可热加载字段检测：对比新旧 `listen_addr` 与 `usage_database`，变化时 `warn!` 提示需重启，其余字段照常生效
- [x] 3.3 单元测试：合法配置 reload 后 providers/routes/pricing/gateway_keys 生效；非法 TOML / 校验失败保留旧快照；`listen_addr`、`usage_database` 变化仅警告

## 4. POST /reload 端点

- [x] 4.1 在 `router.rs` 注册显式路由 `POST /reload`（优先于 fallback），复用 `is_authorized` 鉴权；成功返回 200 摘要，失败返回 500 与错误原因（不含敏感值）
- [x] 4.2 集成测试：带合法 key 触发 reload 成功并生效；坏配置返回 500 且旧配置继续服务；无 key 返回 401 且不触发 reload

## 5. 文件监听

- [x] 5.1 实现 `spawn_config_watcher(shared, config_path)`：`notify` 监听配置文件所在目录、按文件名过滤事件、~500ms 防抖合并后调用 `reload()`；所有错误捕获并记日志，watcher 创建失败仅 warn 不阻止启动
- [x] 5.2 `main.rs`：启动时创建共享状态并 spawn watcher 后台任务
- [x] 5.3 手动验证（Windows）：运行网关，编辑配置文件保存，确认日志显示 reload 生效；写入坏配置确认旧配置继续服务

## 6. 收尾

- [x] 6.1 更新示例配置/文档注释，说明可热加载字段、不可热加载字段（需重启）与 `POST /reload` 用法，含 gateway_keys 无缝轮换步骤（先加新 key → reload → 切换 → 删旧 key → reload）
- [x] 6.2 `cargo fmt`、`cargo clippy`、全量 `cargo test` 通过
