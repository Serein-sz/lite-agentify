# lite-agentify

轻量级 LLM API 网关:一个 Rust 二进制,统一 OpenAI 与 Anthropic 协议入口,内置模型目录路由、多上游故障转移、账号与 API 密钥、预付费额度计费,以及 Web 管理台。

适合个人与小团队自建网关:把手里的多个上游(官方 API、兼容端点、中转)聚合成一套自己的模型名与密钥体系,统一鉴权、记账、限额与分发。

## 功能特性

- **模型目录路由** — 客户端只见模型名;管理员为每个模型编排有序部署链(provider × 上游模型名),按序故障转移;限流时先对同一 provider 退避重试再切换
- **双协议入口** — OpenAI(`/v1/chat/completions`、`/v1/responses`)与 Anthropic(`/v1/messages`);流式(SSE)与非流式均透传,并从响应中解析 token 用量计费
- **账号与密钥** — `admin` / `user` 两种角色;API 密钥仅存 SHA-256 哈希,支持限定可调用模型与累计消费上限
- **预付费额度** — 管理员充值(USD),余额 = 累计充值 − 累计消费;余额耗尽返回 402(软限);计数器每 60 秒与 Postgres 对账
- **按用量计费** — provider + model 定价规则,支持 `*` 通配回退;用量异步批量落库,请求路径零数据库访问
- **可选 Redis 热层** — 消费计数器、控制台会话、登录锁定;Redis 故障自动降级(会话读取失效关闭),Postgres 始终是唯一真相
- **Web 管理台** — 与代理同端口的 `/admin`,React SPA 编译期嵌入二进制,中文界面
- **配置热重载** — 文件监听 + `POST /reload`;替换原子化,失败时旧配置继续服务

## 架构总览

```
 客户端(API 密钥)──▶  lite-agentify(单二进制,默认 :3000) ──▶ 上游 A / 上游 B / …
 浏览器 ──────────▶  /admin 管理台                          按模型部署链故障转移
                              │
               ┌──────────────┴──────────────┐
               ▼                             ▼
      PostgreSQL(唯一真相)           Redis(可选热层)
   账号 · 密钥 · 模型目录 · 定价      消费计数器 · 会话 · 登录锁定
   额度台账 · 用量记录               (故障自动降级,不存真相)
```

三条原则:

1. **Postgres 是唯一真相**。账号、密钥、模型目录、定价、额度台账、用量都在库里,启动时自动迁移建表。
2. **请求路径不碰数据库**。进程内维护 arc-swap 只读快照,鉴权、路由、限额检查全部在内存完成;管理台的变更先落库、再重建快照原子替换,失败时旧快照继续服务。
3. **Redis 只放派生或会过期的状态**。丢掉 Redis 最多损失控制台会话和一个对账周期内的计数精度,不丢账。

## 快速开始

### 前置条件

- **PostgreSQL**(硬依赖;网关启动时自动执行迁移,无需手工建表)
- 源码构建:Rust 工具链(edition 2024,≥ 1.85)+ Node 22 / pnpm 9(管理台);或者直接用 Docker

### 1. 编写配置文件

配置路径取 `LITE_AGENTIFY_GATEWAY_CONFIG` 环境变量,未设置时默认 `~/.config/lite-agentify/lite-agentify.toml`(Windows 取 `%USERPROFILE%`)。必须是 `.toml` 文件:

```toml
listen_addr = "0.0.0.0:3000"   # 可省略,默认即此值

# 首启引导管理员密码:明文写一次,启动后网关自动将其替换为 argon2id 哈希
admin_password = "choose-a-strong-password"

# 必填:PostgreSQL
[database]
url = "postgres://user:password@127.0.0.1:5432/lite_agentify"
max_connections = 5

# 可选:Redis 热层(见「Redis」一节),没有也能跑
# [redis]
# url = "redis://:STRONG-PASSWORD@127.0.0.1:6379/0"
```

> 配置文件里有数据库(和可选 Redis)连接串,属于机密文件:不要提交进版本库,文件权限只授予网关运行账户。

### 2. 构建与启动

**源码构建:**

```bash
cd ui && pnpm install && pnpm build && cd ..   # 先构建管理台,rust-embed 会把 ui/dist 打进二进制
cargo build --release
LITE_AGENTIFY_GATEWAY_CONFIG=/path/to/lite-agentify.toml ./target/release/lite-agentify
```

**Docker:**

```bash
docker build -t lite-agentify .
docker run -d --name lite-agentify \
  -p 3000:3000 \
  -v /opt/lite-agentify:/config \
  lite-agentify
```

镜像已内置 `LITE_AGENTIFY_GATEWAY_CONFIG=/config/lite-agentify.toml`,把配置文件放进挂载目录即可。**挂载目录需可被容器内 uid 10001 写入**——首启会把 `admin_password` 的哈希回写进配置文件。仓库自带的 `docker-compose.yml` 是「复用已有 Postgres 容器 + 外部网络」的部署示例,按自己的环境修改。

### 3. 首次启动会发生什么

- `users` 表为空时,用 `admin_password` 种入 `admin` 账号,并把配置文件里的明文替换为 argon2id 哈希(保留注释与格式;文件只读时告警并改用内存哈希)。此后该字段不再生效,改密码走控制台。
- 自动执行全部数据库迁移。
- 若存在旧版配置字段(`[[providers]]`、`[[pricing]]`、`gateway_keys`、`[[routes]]`),做一次性导入,见「[从旧版本迁移](#从旧版本迁移)」。

### 4. 上架第一个模型

浏览器打开 `http://<host>:3000/admin`,用 `admin` + 上面设置的密码登录,依次:

1. **Provider** — 新建上游:协议(openai / anthropic)、`base_url`、上游 API key
2. **定价** — 为它添加定价规则(支持 `*` 通配;模型的每个部署都必须能命中定价规则才允许上架)
3. **模型** — 新建对外模型名,编排部署链(provider + 上游模型名,顺序即故障转移顺序),然后启用
4. **额度** — 给用户充值:**所有用户初始余额为 0**,不充值时请求会收到 402
5. **密钥** — 创建 API 密钥,明文 `la-…` **只显示一次**;可选限定可调用模型、设置累计消费上限

### 5. 发起调用

```bash
# OpenAI 协议
curl http://<host>:3000/v1/chat/completions \
  -H "Authorization: Bearer la-..." \
  -H "Content-Type: application/json" \
  -d '{"model": "你的模型名", "messages": [{"role": "user", "content": "你好"}]}'

# Anthropic 协议
curl http://<host>:3000/v1/messages \
  -H "x-api-key: la-..." \
  -H "anthropic-version: 2023-06-01" \
  -H "Content-Type: application/json" \
  -d '{"model": "你的模型名", "max_tokens": 128, "messages": [{"role": "user", "content": "你好"}]}'

# 列出该密钥可调用的模型
curl http://<host>:3000/v1/models -H "Authorization: Bearer la-..."
```

## 端点一览

| 方法 | 路径 | 鉴权 | 说明 |
| --- | --- | --- | --- |
| POST | `/v1/chat/completions` | API 密钥 | OpenAI Chat Completions 代理 |
| POST | `/v1/responses` | API 密钥 | OpenAI Responses 代理 |
| POST | `/v1/messages` | API 密钥 | Anthropic Messages 代理 |
| GET | `/v1/models` | API 密钥 | 该密钥可调用的已上架模型;请求带 `anthropic-version` 头时返回 Anthropic 形状,否则 OpenAI 形状 |
| GET | `/healthz` | 无 | 健康检查 |
| POST | `/reload` | API 密钥 | 热重载配置文件 |
| — | `/admin`、`/admin/api/*` | 登录会话 | 管理台页面与管理 API |

API 密钥通过 `Authorization: Bearer la-…`、`x-api-key` 或 `api-key` 头携带。只有上表中的路径会被代理,其他路径(如 `/v1/embeddings`)不路由。

## 管理台

与代理同端口,路径 `/admin`。前端是 pnpm + Vite + React SPA(`ui/`),release 构建时由 rust-embed 嵌入二进制,无需单独部署。

| 页面 | 可见角色 | 功能 |
| --- | --- | --- |
| 仪表盘 | 全部 | 本人余额三卡(余额 / 累计充值 / 累计消费);请求、token、成本、延迟、错误率统计与明细(user 只看到自己的) |
| 密钥 | 全部 | 创建 / 吊销密钥,编辑可调用模型与消费上限(admin 可管理全部密钥) |
| 修改密码 | 全部 | 自助改密 |
| 用户 | admin | 建号、禁用、重置密码 |
| 额度 | admin | 充值(含负数冲正)、余额总览、台账 |
| 模型 | admin | 模型目录与部署链编排、上架 / 下架 |
| Provider | admin | 上游管理;密钥掩码显示、单个揭示 |
| 定价 | admin | 定价规则管理 |

## 模型目录与路由

客户端调用的是**模型**,不是 provider;目录即路由契约:

- 每个模型持有一条管理员编排的有序**部署链**:`(provider, 上游模型名)` 序列。网关按序尝试,每次尝试把请求体的 `model` 改写成该部署的上游名;传输错误与 5xx 立即切换下一个部署,限流状态先对同一 provider 退避重试(见「[限流重试](#限流重试)」)。
- 一个模型可以同时拥有 OpenAI 与 Anthropic 协议的部署;每个请求只使用与其端点协议一致的部署。
- 请求在接触任何上游前全部在内存中解析:未知 / 停用的模型 → 协议原生 404;密钥无权调用 → 403;该端点协议下没有部署 → 404 并提示哪个协议可用。请求体必须含字符串 `model` 字段。
- **上架(enabled)前置条件**:模型的每个部署都能解析到定价规则(通配符算数)。上架未定价的模型、把已上架模型的链改成未定价状态、删除 / 收窄已上架模型依赖的定价规则,都会被 409 拒绝。停用状态的模型是草稿,不受此约束。
- 上游 provider 的原生模型列表永不对外暴露;`GET /v1/models` 由网关按密钥可见范围自答。

## 账号与 API 密钥

- **admin** — 管理用户(创建 / 禁用 / 重置密码)、Provider、定价、模型目录与额度,可见全部用量。
- **user** — 自助:创建 / 吊销自己的密钥、改自己的密码、看自己的用量与余额。

密钥要点:

- 创建时明文(`la-…`)只显示一次,库里只存 SHA-256 哈希与 `la-…` 展示前缀。
- 吊销密钥或禁用其所属用户,在下一次快照刷新后立即停止认证。
- `allowed_models`(可选,本人或 admin 可改):限定密钥可调用的模型,越权调用 403;其 `GET /v1/models` 列表也只显示可调用的。不限定则可调用全部已上架模型。
- `spend_cap_usd`(可选,本人或 admin 可改):密钥累计消费上限,达到后该密钥请求收到 402,独立于所属用户余额和用户的其他密钥。

## 额度与计费(预付费)

消费是**预付费、纯累计**的:管理员给用户充值(USD),每个请求的估算成本计入累计消费。没有账期、没有重置——**余额 = 累计充值 − 累计消费**,永远由台账推导,不落盘存储。

- **充值**是管理员操作:控制台「额度」页或 `POST /admin/api/credits/grants`(`{"user_id", "amount_usd", "note"}`)。负数金额用于冲正。`credit_grants` 台账只追加,记录谁、充了多少、为什么;用户在仪表盘看到自己的余额(`GET /admin/api/me/balance`)。
- **限额是前置且软性的**:网关在接触上游前检查内存计数器(请求路径零数据库访问)。用户累计消费达到累计充值 → 协议原生 `402`,错误码 `insufficient_quota`;密钥达到 `spend_cap_usd` → 同样 402 并指明是密钥上限。`429` 保留给上游限流。
- **软限语义**:成本在响应完成后计入,所以并发在途请求可能让余额超支它们自身的成本。计数器每 60 秒与 Postgres 重算值对账(同时治愈内存模式的崩溃丢失与 Redis 漂移)。这是有意的取舍:可用性优先于精确截断。
- **上线提醒**:用户初始余额为 0,请求会 402——先充值再发密钥。

## Provider 与定价管理

Provider、定价规则与模型目录都存 PostgreSQL,通过管理台(或 `/admin/api/providers`、`/admin/api/pricing`、`/admin/api/models`)管理,不在配置文件里。变更即时生效(快照重建),不需要重启或改文件。

- **Provider**:id、协议、base URL、上游 API key、可选 anthropic-version。上游 key 在列表 / 详情里掩码显示,通过 `POST /admin/api/providers/<id>/reveal` 一次揭示一个。删除仍被模型部署引用的 provider 会被 409 拒绝并指明模型。
- **定价**:provider(或 `*`)× model(或 `*`),每百万 token 价格与币种。通配回退顺序:provider+model → provider+`*` → `*`+model → `*`+`*`。删除或收窄**已上架**模型依赖的规则会被 409 拒绝。

## 用量记录

每条用量记录归属到发起请求的用户与密钥;user 在控制台只能看自己的,admin 全量可见。定价是部署方维护的配置,网关不抓取上游价格、不硬编码模型价格。

用量异步写入:代理把记录交给后台批量写入器,响应路径从不等待数据库。仪表盘因此是最终一致的——刚完成的请求最多延迟一个刷写间隔(约 1 秒)出现。优雅停机(Ctrl-C / SIGTERM)会先排空缓冲再退出;硬杀(SIGKILL)可能丢掉内存缓冲——用量记录是尽力而为的,永不阻塞或失败客户端响应。

## Redis(可选热状态后端)

不配 Redis 时以下状态都在进程内存里,单实例完全够用。加上 `[redis]` 段并重启,热状态迁入 Redis:

```toml
[redis]
url = "redis://:STRONG-PASSWORD@127.0.0.1:6379/0"   # 机密,与 database.url 同级对待
```

迁入内容:

- **消费计数器**(`spent:user:{id}`、`spent:key:{id}`)——网关重启不清零。Redis 故障期间降级为内存影子(请求继续服务,每个故障窗口只告警一次),恢复后由对账循环用 Postgres 真相重新播种。
- **控制台会话**(`session:{token}`,原生 24h TTL)——登录态在网关重启后仍有效。Redis 故障期间会话读取**失效关闭**:控制台一律 401 直到 Redis 恢复,认证绝不失效开放。
- **登录锁定**(`lockout:{username}`,TTL 即锁定窗口)。
- 预留的 **`config_changed` 发布 / 订阅频道**:影响快照的控制台变更会向它发布。本单实例版本的订阅方刻意为空操作(变更实例已刷新自己的快照);频道的存在让未来多实例扇出不需要协议变更。

该段与 `[database]` 一样仅重启生效。Postgres 仍是一切的真相——Redis 只放派生或会过期的状态,丢掉它的代价至多是活跃控制台会话与一个对账间隔的计数新鲜度。

**安全**:会话令牌在 Redis 里,拿到 Redis 访问权等于拿到管理台。设置强 `requirepass`(替换任何弱密码或默认密码),绑定内网地址并做好端口防火墙。`url` 携带密码,配置文件因此是机密存储——不进版本库,只授权网关运行账户可读。

## 配置文件参考

完整示例。providers、定价、模型、账号、密钥**不在**文件里——它们存数据库、由控制台管理(见上文):

```toml
# 监听地址。改动需重启。
listen_addr = "0.0.0.0:3000"

# 仅首启引导 admin 用户;启动后被自动替换为 argon2id 哈希。
admin_password = "choose-a-strong-password"

# 必填。改动需重启;[usage_database] 是本段的废弃别名,不可 enabled = false。
[database]
url = "postgres://user:password@host:5432/dbname"
max_connections = 5

# 可选热层。改动需重启。
# [redis]
# url = "redis://:STRONG-PASSWORD@host:6379/0"

# 可选,热重载生效;缺省即以下默认值。
[retry]
retryable_statuses = [429, 529]  # 触发同 provider 退避重试的上游状态码
max_attempts = 4                 # 每个 provider 的总尝试次数(含首次,>= 1)
base_delay_ms = 1000             # 首次退避等待;之后向 max_delay_ms 增长
max_delay_ms = 8000              # 单次等待上限,也封顶过大的 Retry-After
```

### 配置热重载

网关运行中重载配置文件,无需重启。两个触发方式共用同一套重载逻辑:

- **文件监听**:网关监听启动时解析出的那个配置文件,保存后自动重载(约 500ms 防抖)。
- **端点**:`POST /reload` 带 API 密钥,如 `curl -X POST -H "Authorization: Bearer <api-key>" http://<listen_addr>/reload`。成功 200,失败 500 并附原因。

行为:

- 可热重载的文件字段:`retry`(文件中仅剩的可热重载段)。
- 不可热重载:`listen_addr` 与 `database`——改动被忽略并告警,需重启;其余字段照常生效。
- Provider、定价、模型目录、账号与密钥在数据库里:由各自的管理 API 触发快照刷新即时生效,与文件重载无关。
- 新配置解析或校验失败时,旧配置继续服务并记录错误;替换是原子的,请求不会看到半套配置。
- 在途请求以它开始时的配置快照跑完。

### 限流重试

上游返回限流状态时,网关等待后对**同一** provider 重试若干次,然后才推进故障转移链。针对的是最常见的可恢复上游错误:短暂的 429/529,退避一下通常就能过;立刻切换反而浪费主力 provider,还会锤打可能同样被限的备用。

- 命中 `retryable_statuses` → 等待后重试同一 provider,总计 `max_attempts` 次;用尽后才推进到链上下一个 provider。
- 等待时长优先取响应的 `Retry-After` 头(秒数或 HTTP 日期形式),封顶 `max_delay_ms`;否则用带全抖动的指数退避(`base_delay_ms` 向 `max_delay_ms` 翻倍),避免对被限 provider 的惊群。
- 传输错误与 HTTP 5xx 仍**立即**切换,不做同 provider 重试。其余 2xx/3xx/4xx 响应原样转发给客户端。
- 链尾 provider 重试后仍被限流时,把真实的限流响应(含 `Retry-After`)转发给客户端,而不是合成 502。

## 安全注意事项

- 登录只收用户名和密码;未知用户、被禁用用户、密码错误的失败响应完全一致,不泄露用户名存在性。
- 登录成功设置 `HttpOnly`、`SameSite=Strict`、作用域 `/admin` 的会话 Cookie(24h TTL)。会话默认在内存——重启网关等于全员登出(热重载不会)——配 `[redis]` 后会话跨重启存活,且 Redis 故障期间失效关闭。
- 同一用户名连续失败 5 次锁定 60 秒;配合 argon2 的慢验证让网络爆破不可行,单个攻击者也无法锁定其他用户。
- 管理 Provider 即托管上游 API key(改 `base_url` 就能重定向流量)。上游 key 存数据库、响应中以 `__MASKED__…` 掩码、逐个揭示;把端口暴露到 localhost / 内网之外要三思并配防火墙。
- 上游 provider key 在数据库中**明文**存储,靠数据库访问控制保护——拿到数据库访问权等于托管全部上游凭据。
- 配置文件携带 `database.url`(及可选 Redis url),本身就是机密存储:不进版本库,权限最小化。
- 目录类变更在快照替换前校验:会破坏服务的变更(如部署引用已删除的 provider)落库但旧快照继续服务,响应附带警告。

## 从旧版本迁移

三类旧配置在首启时做一次性导入,之后文件里的残留字段只产生启动告警,可从容删除:

- **`[[routes]]` + provider `model_aliases` → 模型目录**:`models` 表为空且文件有 `[[routes]]` 时,按路由推导目录——每个 provider 的别名 `(公开名 → 上游名)` 成为模型 `公开名` 在链上相应位置的部署。定价完整的模型直接上架,其余为停用草稿(补定价后在控制台上架)。链上**没有**别名的 provider 意味着无法枚举的透传模型——逐个记日志提醒手工建目录。本版本保留数据库里的别名数据以便回滚旧二进制,下一版本移除。
- **`gateway_keys` → API 密钥**:`api_keys` 表为空时,文件里的静态 key 一次性导入为 bootstrap admin 名下的有效密钥,存量客户端不断线。确认后在控制台创建按人密钥并删除该字段。
- **文件 `[[providers]]` / `[[pricing]]` → 数据库**:对应表为空时一次性导入,之后文件段被忽略(启动告警)。导入不改写这些文件段,回滚旧二进制仍可工作。

**破坏性变更**(相对旧的路径前缀路由版本):未知模型不再透传上游,目录即权威,客户端必须发送已编目的模型名;只代理固定协议端点(加网关自有的 `/v1/models`、`/healthz`、`/reload`、`/admin`),自定义路径前缀与非模型端点(如 `/v1/embeddings`)不再路由;请求体必须含字符串 `model` 字段。

## 开发

**后端:**

```bash
cargo build          # 无需先构建前端(ui/dist 只有 .gitkeep;debug 下控制台显示“资源未构建”提示页)
cargo test           # 全部测试;Redis 集成测试默认跳过
LITE_AGENTIFY_TEST_REDIS_URL=redis://127.0.0.1:6379/0 cargo test   # 连真实 Redis 跑门控测试
```

**前端(管理台):**

```bash
cd ui
pnpm install
pnpm dev             # Vite 于 :5173 服务,并把 /admin/api 代理到本地网关,无需重编译 Rust
pnpm build           # 产出 ui/dist,release 构建时嵌入二进制
```
