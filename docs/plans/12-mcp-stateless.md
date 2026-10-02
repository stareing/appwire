# 12 MCP 无状态协议：会话状态迁移、`_meta` 键名与错误码分区（方案）

> 状态：方案（2026-10-02）；S1、S3、S4、S5、S6 已实施，S2 部分完成（见第 6 节各记录）。
> 与 `docs/plans/12-mcp-2026-07-28.md`（下称「12 迁移计划」）的分工：变更全表（M1–M9、m1–m10）、rmcp 能力核查、传输与版本路由
> 以 12 迁移计划为准，本文件不重复定义；本文件只负责**依赖 MCP 会话的行为如何迁移**、`_meta` 键名、错误码分区三件事。
> 本文件第 3 节与 12 迁移计划 3.2 表不一致处（见 3.6），以本文件为准，建议主会话在 12 迁移计划 3.2 / m10 加指向本文件的说明。
> 标注：`[S]` 官方规范（附 URL，本次 2026-10-02 抓取），`[S*]` 引自 12 迁移计划已核实的来源（本次未重取），`[R]` rmcp 3.5.0 源码
> （`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/rmcp-3.5.0/src/`），`[L]` 本仓库代码，**推断**写明依据，**假设**写明验证方法。

## 0. 结论

1. **需要改 Hub 内部与 spec，不需要改 SDK ↔ Host 协议（`crates/protocol` 消息、`app/*` 方法），不需要升级 rmcp。**
   Hub API 只新增（可选配置字段、`ApprovalRequest` 可选字段），不改已有签名。
2. **会话状态的新载体分三层**（第 3 节）：
   - legacy 客户端（`initialize`）：保持 `mcp:<n>` 会话，行为完全不变；
   - modern 请求：**调用方主体（principal）**——由请求凭据决定（本机令牌 / IPC 同用户），在第 16 项 N5 按 Agent 发令牌之前所有 modern 请求共用一个主体；
     规范明确允许“按请求出示的授权”区分列表，并把“迁到显式句柄或认证主体”列为会话状态的迁移路径（`[S]` SEP-2567）；
   - 需要多份并存的状态（第 16 项 P1 任务、句柄）：**显式句柄放在工具参数 / 结果里**，不放 `_meta`（`_meta` 由客户端填写，模型写不进去）。
   - `_meta` 只承载**单次请求**的控制信息（截止时间、幂等键、追踪、进度令牌），不承载跨请求状态。
3. **modern 下 `tools/list` 只能随服务器状态与请求凭据变化，不能随其他请求的副作用变化**（`[S]` SEP-2567）——
   渐进暴露的“展开 / 调用 / `apps.select` 后加入列表”与总览“首次附带”在 modern 下必须取消或改写；`tools/list` 结果另须 `ttlMs` / `cacheScope`（12 迁移计划 m5）。
4. **`_meta` 键名：`app-mcp/` 改为 `dev.appwire/`**（第 4 节；机主 2026-10-02 选定品牌名前缀，S1 已实施）。`app-mcp/` 语法合法、不在保留区，不违反 MUST；但规范 SHOULD 用反向域名，
   `app-mcp` 是通用词，与他人撞名风险高。`dev.appmcp` 与仓库已有的反向域名标识（Android `dev.appmcp.action.WAKE`、D-Bus `dev.appmcp.App`、
   Kotlin `dev.appmcp.hub`）一致，且第二个标签是 `appmcp` 而非 `mcp`，不触发保留规则。当前无发布版本（`git tag` 为空），改名成本只在 Hub 一处。
   AppWire 的结果状态（`pending` 等）**不能**做成 `resultType` 取值，继续放 `_meta`（第 4.3 节）。
5. **错误码：规范对 -32000…-32019 的约束比 12 迁移计划 m10 写的更严**——新码 MUST NOT 分配在此区、新实现 SHOULD NOT 使用、
   接收方 MUST NOT 对其（-32002 除外）假定含义（`[S]` basic/index）。影响：① 12 迁移计划 m10「新增数值码只能用 -32016～-32019」与规范冲突，
   应作废（`spec/protocol.md` 第 4 节的 -31001 规则已正确）；② Hub 把**上游 MCP 服务器**的 -32001…-32019 按本协议含义反查（`mcp_error_to_tool`）违反
   MUST NOT，需修复；③ modern 出口的 JSON-RPC 错误不再发 -320xx 业务码，改为 `-32602` / `-32603` + `data.kind`。SDK ↔ Host 协议不是 MCP，码值保持不变（E-06）。
6. 实施拆为 S1…S8（第 6 节），S1–S3 不改变默认行为可先做，S4–S7 与第 16 项 P1、12 迁移计划第二部分同批。

## 1. 现状：依赖 MCP 会话的行为（事实，`[L]`）

| # | 行为 | 证据（文件:行） | 依赖会话的方式 |
|---|---|---|---|
| H1 | 每个 MCP 连接一个 `McpSession`，会话键 `mcp:<id>`、日志 `cid = mcp-<id>` | `crates/hub/src/mcp.rs:32-50` | 身份 = 连接 |
| H2 | `McpSession` 析构时移除 peer、订阅，并 `drop_session_state`（含收回租约） | `mcp.rs:53-59`；`hub.rs:661-686` | 会话结束 = 清理信号 |
| H3 | `on_initialized` 才登记 peer；`list_changed` 按登记的 peer 广播 | `mcp.rs:105-109`；`hub.rs:620-659` | 依赖 `initialize` 握手 |
| H4 | 只声明到 2025-11-25；`instructions` 只在 `initialize` 结果中送达 | `mcp.rs:81-103` | 握手 |
| H5 | 会话状态表 `SessionState { selected, delivered, leases, exposed }` | `hub.rs:261-270` | — |
| H6 | `apps.select` 选择按会话保存，路由 / 资源读取 / 导航 / 激活优先用它，`apps.list` 显示合并后的选择 | `call.rs:977-995`；`hub.rs:687-709`；`call.rs:372, 587, 1096`；`agent_control.rs:119, 144` | 会话内跨请求状态 |
| H7 | 总览“首次附带”：每会话首次接触某 App 时附在结果最前，按 (appId, 版本) 去重 | `hub.rs:711-729`；`call.rs:340, 382, 399`；`spec/protocol.md` 7.2 | 跨请求去重 |
| H8 | 渐进暴露：`tools/list` = 内置 + 本会话展开 / 调用 / 选定过的 App；新增时只向该会话发 `list_changed` | `hub.rs:770-831`；`call.rs:423-426, 990-993`；`spec/hub-api.md` 3.7 | **列表随会话与其他请求的副作用变化** |
| H9 | 租约按（会话, App）发放与统计；会话结束收回；`apps.activate` / `apps.release` 作用于“本会话” | `lifecycle.rs:395-447`；`lease.rs:1-13, 186-197`；`call.rs:583, 778`；`agent_control.rs:152, 167-179` | 会话结束 = 收回信号 |
| H10 | 请求流空闲收回：按会话键统计进行中请求与最近活动 | `lifecycle.rs:400-405, 499-505`；`lease.rs:9-10` | 会话键（不依赖连接，可换键） |
| H11 | 资源订阅：`resources/subscribe` 按 MCP 会话号保存，`updated` 发给会话 peer | `mcp.rs:202-228`；`hub.rs:293-295, 1072-1100, 1189-1215` | 会话号 |
| H12 | 进度：经本请求的 `context.peer` 发 `notifications/progress` | `mcp.rs:61-77, 130` | 只依赖本请求（modern 可用） |
| H13 | 取消：`context.ct` → `tools/cancel` | `mcp.rs:143-147` | 只依赖本请求（断流取消待验证，12 迁移计划 m-cancel） |
| H14 | 审批请求带 `session: mcp:<n>` | `call.rs:475`；`types.rs:614-626` | 会话键仅作显示 / 厂商判断 |
| H15 | `/status` 的 `mcp_sessions` 计数 | `hub.rs:581` | 会话数 |
| H16 | stdio 出口一个进程一个 `McpSession` | `hub.rs:1981-1985` | 进程 = 会话 |
| H17 | HTTP `/mcp` 用 `LocalSessionManager` + 默认配置（`legacy_session_mode = true`） | `http_server.rs:51-66` | 传输层会话 |
| H18 | 请求 `_meta` 的 `app-mcp/timeoutMs`、`app-mcp/idempotencyKey`；结果 `_meta` 的 `app-mcp/status`、`stateResource`、`routedTo` | `names.rs:1-38`；`request_meta.rs:43-60`；`spec/hub-api.md` 3.15 | 单次请求，不依赖会话 |
| H19 | HTTP 令牌只有一个（本机令牌），无按 Agent 区分的凭据 | `http_server.rs:170-211` | — |
| H20 | 实例路由的“最近活跃 / 聚焦”是全局实例状态，不按会话 | `routing.rs:1-8` | **不依赖会话**（任务说明中的“最近使用 App”在代码中即此项） |

小结：真正依赖会话的是 H1–H3、H6–H11、H14–H16；H12、H13、H18、H20 本身就是按请求或全局，modern 下可直接沿用。

## 2. 规范事实与影响矩阵

### 2.1 规范事实（本次核实）

| # | 事实 | 来源 |
|---|---|---|
| S-F1 | MCP 是无状态协议：服务器不得依赖同一连接上的先前请求建立上下文；跨请求状态 MUST 由客户端每次传入的显式标识引用；连接（包括 stdio 进程）不是会话 | `[S]` https://modelcontextprotocol.io/specification/2026-07-28/basic/index（Statelessness） |
| S-F2 | 结果 MUST 带 `resultType`（`complete` / `input_required`，扩展可经能力声明新增）；客户端遇到不认识的取值 MUST 视为无效 | 同上（ResultType） |
| S-F3 | 错误码：-32000…-32019 为 legacy，新码 MUST NOT 分配、新实现 SHOULD NOT 使用、接收方除 -32002 外 MUST NOT 假定含义；-32020…-32099 归规范，只定义了 -32020/-32021/-32022；新实现 MUST NOT 发 -32002（改 -32602）；新的非规范码 SHOULD 分配在 -32768…-32000 之外 | 同上（Error Codes） |
| S-F4 | `_meta` 键 = 可选前缀 + 名称；前缀为点分标签加 `/`，标签以字母开头、字母或数字结尾、中间可含 `-`；SHOULD 用反向域名；**第二个标签**为 `modelcontextprotocol` 或 `mcp` 的前缀保留；名称首尾为字母数字，中间可含 `-` `_` `.` | 同上（`_meta`） |
| S-F5 | 保留键：`progressToken`、`io.modelcontextprotocol/{protocolVersion, clientInfo, clientCapabilities, logLevel, subscriptionId}`、结果 `io.modelcontextprotocol/serverInfo`、`traceparent` / `tracestate` / `baggage`；第三方扩展用自己的厂商前缀 | 同上 |
| S-F6 | `clientInfo` / `serverInfo` 自报不可信：SHOULD NOT 据此改变行为或做安全决定 | 同上 |
| S-F7 | 删除会话；列表不得按连接变化，也不得因其他请求的副作用变化；**可以**按请求出示的授权（主体 / 范围）变化；跨调用状态用服务器签发的不透明句柄，作为普通工具参数 / 结果传递（不是协议结构）；无鉴权时句柄 ≥128 bit 随机且限时；过期给出可恢复的错误；会话键状态的迁移路径为“显式句柄或认证主体” | `[S]` https://modelcontextprotocol.io/seps/2567-sessionless-mcp（Final） |
| S-F8 | 同一端点可同时服务两代客户端（dual-era） | `[S*]` https://modelcontextprotocol.io/specification/2026-07-28/basic/versioning |
| S-F9 | `subscriptions/listen` 取代 GET 流与 `resources/subscribe`，订阅寿命以该请求为界 | `[S*]` https://modelcontextprotocol.io/specification/2026-07-28/basic/patterns/subscriptions |
| S-F10 | rmcp 3.5.0 把请求 `_meta` 保存为透明 map（`RequestMetaObject(pub MetaObject)`），按键读取保留键；`RequestContext::protocol_version()` 先读 `_meta` 再回退握手信息 | `[R]` `model/meta.rs:386-470`；`service.rs:1232-1250` |

### 2.2 影响矩阵（只列会话相关；其余见 12 迁移计划第 2 节）

| 规范点 | 受影响的现状 | legacy 路径 | modern 路径 | 任务 |
|---|---|---|---|---|
| S-F1 / S-F7 删除会话 | H1、H2、H5、H16 | 不变 | 每请求无连接身份；状态键改为主体键（3.2） | S4 |
| S-F7 列表不随副作用变化 | H8（渐进暴露）、H6 中“选定后列入” | 不变 | 列表 = 内置 + 按服务器状态 / 主体策略的 App 工具（3.3） | S5 |
| S-F1 无跨请求推断 | H7（总览首次附带） | 不变 | 不在调用结果中附带；改经 `server/discover.instructions`、`apps.tools`、`apps.overview`（3.3） | S5 |
| S-F7 句柄 / 主体 | H6（`apps.select`）、H9、H10、H14 | 不变 | 主体键；需多份并存时用句柄（3.2、3.4） | S4、S6 |
| S-F9 订阅 | H3、H11 | 不变 | `listen` 流；订阅者从会话号泛化为订阅 ID（12 迁移计划 M4） | S7 |
| S-F1 stdio | H16 | 一进程一会话 | **不得**以进程为会话：stdio 上的 modern 请求同样走主体键（12 迁移计划 3.1 写“与现状一致”，需更正） | S4 |
| S-F2 `resultType` | 结果 `_meta` 的 `app-mcp/status`（H18） | — | `status` 不得映射为 `resultType`（4.3） | S2 |
| S-F3 错误码 | `to_mcp_error`、`mcp_error_to_tool` | 不变 | 第 5 节 | S3 |
| S-F4 / S-F5 键名 | H18 | 同改（键名与协议版本无关） | 同左 | S1 |

## 3. 兼容层设计

### 3.1 判定与分流

- 判定：`RequestContext::protocol_version()` 无握手版本（`!has_initialize()`）即 modern（12 迁移计划 3.2，`[R] service.rs:1233-1240`）。
- `McpSession` 分两种模式：legacy（`on_initialized` 发生过，现有全部行为）与 stateless（不登记 peer、不建 `SessionState`、析构无副作用）。
  rmcp 对 modern 请求每请求调用一次工厂（12 迁移计划 F9），因此析构清理必须只在 `on_initialized` 之后执行。
- Hub 内部把散落的 `session_key: String` 收敛为一个**调用方键** `CallerKey`（单一定义，D-07）：

| 入口 | `CallerKey` | 寿命 / 回收 |
|---|---|---|
| legacy MCP（HTTP 会话、stdio `initialize`） | `mcp:<n>`（现状） | 会话结束（现状） |
| modern MCP（HTTP、stdio） | `principal:<主体>`；N5 之前固定 `principal:local` | 空闲超时（复用 4e `idle_revoke` 判定与 `MAX_LEASE_SESSIONS` 上限）；没有“结束”信号 |
| Hub API | `api:<session>` / `api`（现状） | `reset_session`（现状） |
| 第 16 项 P1 之后 | 任务对象 ID（见 3.4） | 任务租约到期 |

  主体来自**传输层凭据**，不来自 `clientInfo`（S-F6）：HTTP 为 Bearer 令牌（H19：现在只有一个本机令牌 → 所有 modern 请求同一主体），
  IPC 为同用户连接（同一主体）。第 16 项 N5 若按 Agent 发令牌，主体自然细分，无需再改本层。

### 3.2 各状态的 modern 语义

| 状态（现状） | modern 新语义 | 依据 |
|---|---|---|
| H6 `apps.select` | 写入 `CallerKey` 的选择（N5 前即本机所有 modern Agent 共用），带空闲 TTL；**不影响 `tools/list`**；结果文本写明作用范围与有效期。需要按任务隔离时用 P1 任务句柄（3.4） | S-F7“迁到认证主体”；列表约束 |
| H7 总览首次附带 | 不在调用结果中附带（按主体去重会让同一令牌下的新对话永远拿不到总览——**推断**：去重键只有主体，无法区分对话）；`server/discover.instructions` 给出各 App 一句话简介与“调用 `apps.overview` 查看”提示；`apps.tools` 结果附带该 App 总览 | S-F1；`spec/protocol.md` 7.2 |
| H8 渐进暴露 | 列表 = 内置 `apps.*` + 全局选定（`Hub::select_instance`，厂商 / 用户经 Hub API 设置，属服务器状态）的 App；未生效时全部列出；`apps.tools` 只返回 schema 不改列表；按全名调用未列出的工具照常路由（是否被客户端允许见 U3） | S-F7 |
| H9 租约 | 键 = `CallerKey`；无“会话结束收回”，只有 TTL、请求流空闲收回（H10，已按键实现）与 `apps.release` | 4e B2 已按键实现，只需换键 |
| H11 订阅 | `subscriptions/listen` 流；流关闭即订阅结束；legacy 路径不变 | S-F9 |
| H14 审批 `session` | `principal:<主体>`；另附可选 `client_name`（`clientInfo.name`，**仅显示**） | S-F6 |
| H1 / H15 `cid`、计数 | `cid = mcp-r<n>`（每请求）；`/status` 新增 `mcp_listen_streams`、`mcp_modern_requests`（只增字段） | — |

### 3.3 列表规则（单一定义，建议写入 `spec/hub-api.md` 3.7）

modern `tools/list` / `resources/list` 只是以下输入的函数：注册表与 App 连接状态、全局选择、策略 `hide`、**请求主体**、Hub 配置。
不得读取 `SessionState`、`CallerKey` 下的选择或展开记录。由此带来一个对第 16 项 P2 的更正：规范允许按授权主体区分列表（S-F7），
因此**主体级 `hide` 是允许的**（16-agent-os.md 第 88 行“hide 只能全局生效”在有按 Agent 凭据时可放宽）；按 `clientInfo` 区分仍不允许。

### 3.4 与第 16 项 P1（Agent 任务对象）的关系

- P1 的“modern 请求在 `_meta` 带任务 ID”（16-agent-os.md U8）**不可依赖**：`_meta` 由客户端程序填写，模型无法写入；通用客户端（Claude Code 等）
  没有理由填厂商键（**推断**，依据 S-F5“第三方扩展用自己的前缀、在扩展文档中定义”——只有接入方主动实现才会带；U2 实测确认）。
- 建议：**任务 ID 作为显式句柄**，按 SEP-2567 模式经工具结果返回、作为工具参数传回（如 `apps.task.begin` → `taskId`；需要任务作用域的内置工具接受可选 `taskId`）；
  `_meta` 键 `dev.appwire/taskId` 只作为**可选**通道，供自己实现客户端的 Agent 宿主（Hub API 厂商、自研 Agent）使用。两者都没有时退化为 `CallerKey`。
- P1 落地后 `SessionState` 迁入任务对象；`CallerKey` 成为“默认任务”。legacy 会话 = 一会话一任务（与 16 P1 一致）。
- 句柄安全：本机令牌 / IPC 同用户鉴权在前，句柄仍按 S-F7 生成（≥128 bit、限时、数量上限，与第 11 项 LRU·TTL 共用实现）。

### 3.5 兼容矩阵

| 客户端 | 行为 |
|---|---|
| legacy（e2e 客户端 `2025-06-18`、旧版本 Claude Code） | 完全不变 |
| dual-era（Claude Code 2.1.281） | S7 之前：`-32022` → 回退 legacy（12 迁移计划 U1 已实测）；S7 之后：modern 语义 |
| modern-only | S7 之后可用 |
| 回退开关 | `mcp.max_protocol_version`（12 迁移计划 3.1）设为 `2025-11-25` 即恢复现状 |

### 3.6 与 12 迁移计划 3.2 的差异（以本文件为准）

| 项 | 12 迁移计划 3.2 | 本文件 | 理由 |
|---|---|---|---|
| `apps.select` | 返回 `selection` 句柄 + `apps.call` 通用入口 | 主体级选择；句柄只用于 P1 任务；`apps.call` 不引入 | 第 14 项 U2 要求尽量不引入 `apps.call`；App 工具 schema 由 App 定义，Hub 无法给每个工具加句柄参数 |
| 租约键 | 固定 `mcp` | `principal:<主体>`（N5 前等于固定值） | 与 N5 / P1 衔接，不再改一次 |
| stdio modern | 以进程为会话，与现状一致 | 同 HTTP，走主体键 | S-F1 明确连接 / 进程不是会话 |
| 错误码（m10） | 新码只能用 -32016～-32019 | 新码从 -31001 起，见第 5 节 | S-F3 |

## 4. `_meta` 键名方案（回答第 19 项 U3 / R4）

### 4.1 现有键是否合规

| 检查 | `app-mcp/timeoutMs` 等 | 依据 |
|---|---|---|
| 前缀语法（MUST） | 合规：单标签 `app-mcp`，字母开头、字母结尾、中间 `-` | S-F4 |
| 保留前缀 | 不保留：规则看第二个标签，单标签无第二标签 | S-F4 |
| 反向域名（SHOULD） | 不符合 | S-F4 |
| 名称部分 | 合规（字母数字首尾） | S-F4 |

### 4.2 建议

- **前缀改为 `dev.appmcp/`**。理由：① 满足 SHOULD；② 与仓库已有反向域名标识一致（`spec/manifest.md:116` `dev.appmcp.action.WAKE`、
  `spec/naming.md:146` `dev.appmcp.App`、`spec/hub-api.md:624` `dev.appmcp.hub`），P-04；③ 第二标签 `appmcp` ≠ `mcp`，不保留；
  ④ 键属于协议标识符，按 CLAUDE.md 约定沿用工作名而非品牌名 AppWire。备选 `dev.appwire/`（品牌）或 `io.github.stareing/`（可证明归属）由机主决定（U1）。
  **机主决定（2026-10-02）：`dev.appwire/`**。下表与后文的键名已按 `dev.appwire/` 更新。
- 键表（唯一定义仍在 `crates/hub/src/names.rs` 与 `spec/hub-api.md` 3.15，改名只改这两处）：

| 现键 | 新键 | 方向 |
|---|---|---|
| `app-mcp/timeoutMs` | `dev.appwire/timeoutMs` | 请求 |
| `app-mcp/idempotencyKey` | `dev.appwire/idempotencyKey` | 请求 |
| `app-mcp/status` | `dev.appwire/status` | 结果 |
| `app-mcp/stateResource` | `dev.appwire/stateResource` | 结果 |
| `app-mcp/routedTo` | `dev.appwire/routedTo` | 结果 |
| （R4 新增） | `dev.appwire/callId`、`dev.appwire/instanceId`、`dev.appwire/durationMs`、`dev.appwire/woke` | 结果 |
| （3.4 可选） | `dev.appwire/taskId` | 请求 |

- 兼容（E-06）：无发布版本（`git tag` 为空；键只出现在 `crates/hub` 及其测试），可直接改名。为防止已有本地 Agent 配置依赖旧键，
  请求侧**同时接受**旧键一个小版本（两者都在且值不同 → `INVALID_INPUT`），结果侧只写新键；`spec/hub-api.md` 3.15 写明弃用期。
- 追踪：直接用规范保留的 `traceparent` / `tracestate`（S-F5），不另设厂商键（第 11 项）。

### 4.3 与 `resultType` 的关系（第 19 项 U3 后半）

- `resultType` 是核心结果字段，取值只能是规范定义值或已在能力中声明的扩展值，客户端遇到未知值 MUST 视为无效（S-F2）。
- 因此 AppWire 的 `status`（`done` / `pending` / `partial` / `noop`）**不得**放入 `resultType`；`pending` 也不等于 `input_required`
  （后者是 MRTR，要求客户端补输入后重发原请求）。结论：`resultType` 一律 `complete`（rmcp 构造器负责，已核实 rmcp 3.5.0，见第 6 节 S2 记录），
  状态继续经 `dev.appwire/status` + 文本说明（R1 已实现文本），两者正交。

## 5. 错误码分区影响

| 现状 | 规范要求（S-F3） | 结论 / 处理 |
|---|---|---|
| `ErrorKind::code` -32001…-32019，SDK ↔ Host 协议使用（`crates/protocol/src/error.rs:93-101`；`spec/protocol.md` 第 4 节） | 约束对象是 MCP 实现 | SDK ↔ Host 不是 MCP，**码值不变**（E-06）；-32016…-32019 是 2026-10 才分配的（`a15c05d` 等），按本协议自身规则已转向 -31001，不再在该区新增 |
| MCP 出口 `to_mcp_error` 把 `ErrorKind` 码原样作为 JSON-RPC 错误码（资源读取、上游资源未连接） | 新实现 SHOULD NOT 使用 -32000…-32019；MUST NOT 发 -32002 | `call.rs:1618-1626, 1112-1113`。modern：`ResourceNotFound` / `InvalidInput` → `-32602`，其余 → `-32603`，类别放 `data.kind`；legacy 不变。工具调用走 `isError` 结果，不涉及数值码 |
| 上游 MCP 错误按数值反查本协议类别 `mcp_error_to_tool` → `ErrorKind::from_code` | 接收方 MUST NOT 对 -32000…-32019（除 -32002）假定含义 | `call.rs:1591-1613`（调用点 `call.rs:151`、`hub.rs:1897, 2078`），测试 `call.rs:1906-1907` 把上游 `-32004` 断言为 `USER_REJECTED`——**缺陷**。改为：上游错误只认 `-32002` / 资源读取上下文的 `-32602` → `RESOURCE_NOT_FOUND`，其余 → `HANDLER_ERROR`，原码放 `details.upstreamCode`；补回归测试（T-08）。**入口侧已修复（2026-10-02）** |
| 12 迁移计划 m10“新增数值码只能用 -32016～-32019” | 新码 MUST NOT 分配在该区 | 作废；以 `spec/protocol.md` 第 4 节（-31001 起）为准 |
| 第 18 项 U2：是否有“需用户操作”的标准码 | 规范只定义 -32020/-32021/-32022 | 无标准位置；`USER_ACTION_REQUIRED` 保持 AppWire 协议内定义，MCP 出口经工具错误结果 + `data.kind` 传递 |

## 6. 任务拆分、顺序与验收

范围：`crates/hub`（`mcp.rs`、`hub.rs`、`call.rs`、`lifecycle.rs`、`lease.rs`、`agent_control.rs`、`names.rs`、`request_meta.rs`、`http_server.rs`）、
`crates/host`（配置 / CLI）、Hub 绑定（只增字段）、`spec/hub-api.md`、`spec/protocol.md`、`e2e/`。不改 `crates/protocol` 消息与各 SDK。

| # | 内容 | 依赖 | 验收 |
|---|---|---|---|
| S1 | `_meta` 前缀改 `dev.appwire/`（4.2），请求侧旧键弃用期内兼容 | U1 机主定前缀 | 单测：新键、旧键、两者冲突、非法值；`spec/hub-api.md` 3.15 键表更新 |
| S2 | `resultType` 核查与补齐（12 迁移计划 M8 / U7）；R4 结果元信息按新键输出 | S1 | modern 每种结果（工具、空结果、资源、列表）都带 `resultType: complete`；`_meta.dev.appwire/callId` 与日志 `cid` 可对照。**部分完成（2026-10-02）**，见下方 S2 记录 |
| S3 | 错误码：modern 出口码映射；修复上游错误反查缺陷；作废 m10 表述 | — | 回归测试：上游 `-32004` 不再变成 `USER_REJECTED`；modern 资源不存在为 `-32602`、legacy 仍 `-32002` |
| S4 | `CallerKey` 收敛；`McpSession` 分 legacy / stateless；主体键（HTTP 令牌 / IPC）；stdio modern 同样无状态 | 4e 已完成；与 16 P1 同批设计 | 连续 N 个 modern 请求不新增 / 删除 `SessionState`；legacy 与 modern 并发互不影响；租约在 modern 下按空闲收回。**已完成（2026-10-02，与 16 P1 最小任务对象同批）**，见下方 S4 记录 |
| S5 | modern 列表规则（3.3）与总览改经 discover / `apps.tools`；`ttlMs` / `cacheScope` | S4；与 4c F 合并 | modern 下两次 `tools/list` 之间夹任意 `apps.tools` / 调用 / `apps.select`，结果逐字节相同；`server/discover.instructions` 含 App 简介。**已完成（2026-10-02）**，见下方 S5 记录 |
| S6 | `apps.select` 主体级语义与 TTL；审批 `principal` 与 `client_name`；`/status` 新字段 | S4 | modern `apps.select` 后路由命中所选实例、列表不变；TTL 到期后回到默认路由。**已完成（2026-10-02）**，见下方 S6 记录 |
| S7 | `subscriptions/listen`（12 迁移计划 M4）+ 默认放开 2026-07-28 | S1–S6 | Claude Code 2.1.281 实测：Host 日志无 `Mcp-Session-Id`、调用成功、App 上下线后列表刷新；回退开关恢复 legacy |
| S8 | P1 任务句柄（3.4）：`taskId` 工具参数 + 可选 `_meta` 通道 | 16 P1、N5 | 两个任务句柄各自的选择 / 租约互不影响；过期句柄返回可恢复错误 |

**S2 记录（2026-10-02，rmcp 锁定版本 3.5.0，源码 `~/.cargo/registry/src/*/rmcp-3.5.0`，未升级）**

- 事实：rmcp 3.5.0 有 `model::ResultType`（`complete` / `input_required` / `task`，`src/model.rs:866`）；`CallToolResult`、
  `ReadResourceResult`、各分页列表（`ListToolsResult`、`ListResourcesResult` 等）、`CompleteResult`、`GetPromptResult` 带
  `result_type: Option<ResultType>`，构造器与 `Default` 都填 `Some(COMPLETE)`（如 `src/model.rs:1785`、`4061`）；反序列化缺省为 `None`。
- 事实：服务端 handler 在协商版本 < 2026-07-28 时调用 `ServerResult::strip_result_type_for_legacy_peer` 去掉 `complete`
  （`src/handler/server.rs:246-258`、`src/model.rs:4789`），所以 legacy 线上格式不变。
- 事实：Hub 目前只协商到 `LATEST_WITH_INITIALIZE` = 2025-11-25（`crates/hub/src/mcp.rs` `get_info` / `supported_protocol_versions`），
  modern（2026-07-28）客户端尚不能协商成功（S7 才放开），所以 modern 线上 `resultType` 现阶段无法端到端验证。
- 已补齐：Hub 自建的工具、资源、列表结果都经 rmcp 构造器（带 `complete`）；唯一会是 `None` 的是上游 MCP 服务器透传的结果（上游按旧协议回复时没有该字段）。
  `Invocation::to_mcp` 与 `read_upstream_resource` 对缺省值补 `complete`（`CallToolResult` 只允许该值；已有值不改），单测
  `call_meta_keys_and_result_type` 覆盖。R4 结果元信息已按新键输出（第 19 项 R4）。
- 未知（留给 S7）：空结果（`ping`、`resources/subscribe`、`unsubscribe`、`logging/setLevel`）在 rmcp 3.5.0 中是 `EmptyObject {}`
  （`#[serde(deny_unknown_fields)]`，`src/model.rs:77`，经 `ServerResult::empty`），不带 `resultType`；2026-07-28 要求"每个结果"都带时
  这些回复缺该字段。Hub 的 `subscribe` / `unsubscribe` 返回 `()`，无法在 Hub 侧补齐。处理：S7 放开 modern 前核实规范对空结果的要求，
  并检查届时的 rmcp 版本（升级 rmcp 不在本项范围）；在此之前 modern 不会被协商，不影响现有客户端。

**S4 记录（2026-10-02，与第 16 项 P1 同批）**

- 实现：`crates/hub/src/task.rs` 定义 `CallerKey`（`mcp:<n>` / `principal:<主体>` / `api[:<s>]`，带寿命 `UntilEnd` / `UntilIdle`）、
  `Principal`（现只有 `Local`）、`AgentTask`（原 `SessionState` 的四项状态 + Hub 签发的 `task-<128 位十六进制>` ID）与 `TaskTable`；
  `HubShared.session_state` → `agent_tasks`，`CallCtx.session_key: String` → `caller: CallerKey`，`api_session_key` 删除，租约、选择、
  总览附带、渐进暴露全部按 `CallerKey` 取任务。`LeaseBook` 仍按字符串键（纯状态模块不变，键取 `CallerKey::as_str`）。
- 判定（与 3.1 的差异，**以此为准**）：不用 `protocol_version().has_initialize()`——rmcp 的 `server/discover` 生命周期可以选
  2025-11-25（有握手的版本）而不握手，此时 `protocol_version()` 为 2025-11-25 但 rmcp 按无状态处理（`[R] handler/server.rs:57-97`
  `requires_request_metadata` / `uses_legacy_lifecycle`）。改为：本处理器**处理过 `initialize`**（覆盖 `ServerHandler::initialize`，
  按 rmcp 文档的方式先 `set_peer_info` 再 `negotiate_initialize`）且请求 `_meta` 未声明 2026-07-28 及以后版本 → legacy；否则无会话。
  不在 `on_initialized` 判定：rmcp 不等 `notifications/initialized` 就处理后续请求，HTTP 上两者无先后保证。
- 无会话处理器：不登记 peer、不建会话状态、Drop 无副作用；`apps.select` / `apps.tools` 不发 `list_changed`（无 peer）；审批
  `ApprovalRequest.session = "principal:local"`。stdio / `serve_mcp_stream` 上不握手的客户端同样走主体键（3.6 差异表"stdio modern"）。
- 主体：HTTP 令牌、IPC 同用户、stdio 父进程现在都映射为 `principal:local`（`Principal::Local` 的 @why）；没有把主体塞进 HTTP 请求扩展
  的管道——只有一个取值时无法测试，留给 N5（按 Agent 发令牌时在 `http_server` 核对令牌处得出主体，`McpSession::caller` 读取）。
- 任务回收：`HubConfig.task_idle_ttl`（默认 10 分钟，`0` = 不回收），复用 `LeaseBook` 的请求流活动（新增 `LeaseBook::activity`），
  在既有 `lease_idle_loop` 中计算到期，无按空闲回收的任务时不设定时器；回收时先丢弃已到期的租约记录（不再发 `ttlMs: 0`），再按
  "任务结束"收回其余租约、删除统计（计入 `revoked_session_end`）。
- 外部行为：默认配置下 legacy 客户端（`initialize`）完全不变（全部既有测试与 e2e 不改即通过）；唯一变化是不握手的
  `server/discover` 客户端（以 ≤ 2025-11-25 版本经 stdio / 流）从"每连接一个会话"变为共用主体任务——这正是 S4 的要求。
  `/status`、`HubStatus` 不变（`LeasePairStatus.session` 可出现 `principal:local`）。
- 测试（`crates/hub/src/mcp.rs` `tests`，rmcp 客户端 `ClientLifecycleMode::Discover` 走真实线路，同时回答 U4 的前半：无会话请求经
  rmcp 原样到达 handler）：`stateless_requests_share_one_principal_task`（同一连接与每请求一个新处理器，共 13 个请求，任务数与任务 ID
  不变、Drop 无副作用、租约统计键为 `principal:local`）、`legacy_and_stateless_do_not_affect_each_other`（并发调用；选择与
  `apps.release` 互不影响；legacy 断开只结束自己的任务）、`stateless_leases_and_task_reclaimed_by_idle`（默认租约按 `idle_revoke` 收回、
  主体任务按 `task_idle_ttl` 回收、legacy 任务保留、回收后新任务无旧选择）；`task.rs` 单测。T-10：临时把 Drop 改为无条件清理、
  把 `caller()` 改为总是 legacy、去掉 `expire_idle_tasks` 调用，三组测试分别失败。
- 未做（留给后续项，范围不变）：S5 modern 列表规则（现在无会话请求的 `tools/list` 仍按主体任务的渐进暴露记录变化，违反 S-F7，S5 改）、
  总览首次附带按主体去重（S5 改经 discover / `apps.tools`）；S6 `apps.select` 主体级 TTL（现在随任务空闲回收一起失效）、审批
  `client_name`、`/status` 新字段；S7 放开 2026-07-28 与 `subscriptions/listen`；S8 任务句柄（`AgentTask.id` 已就绪）。

**S5 记录（2026-10-02）**

- 列表规则（3.3 的实现）：`HubShared::exposed_apps` 对无会话调用方（`CallerKey::is_stateless`，即 `principal:<主体>`）只读全局选择，
  不读任务上的 `exposed` / `selected`；`expose_app` 对其不记录、恒为 `false`；`mcp_tools` 因此是注册表、全局选择、策略 `hide`、配置的函数。
  legacy 会话与 Hub API 的渐进暴露代码路径不变。
- U3 的保守处理：新增 `HubConfig.stateless_tool_exposure`（默认 `All`）——无会话请求默认全部列出；设为 `Progressive` / `Auto` 时列表 =
  内置（含 `apps.tools`）+ 全局选定的 App，`apps.tools` 只返回定义。legacy 仍按 `tool_exposure`。`app-mcp-host` 配置 / 命令行与各绑定
  未加此项（本次范围外；默认值即保守值，U3 验证后再决定是否开放）。
- 缓存提示：rmcp 3.5.0 支持（`[R] model.rs:1750-1805` 分页结果的 `ttl_ms` / `cache_scope` 与 `with_ttl_ms` / `with_cache_scope`；
  `ReadResourceResult` 同，`model.rs:1912-1955`；`DiscoverResult` 的两字段为必填，默认 `0` / `private`，`model.rs:1308-1410`）。
  rmcp 回复旧版本客户端时只去掉 `resultType: complete`，**不去掉**这两个字段（`[R] model.rs:4789`），所以只对无会话请求设置：
  `tools/list`、`resources/list`、`resources/templates/list`（新覆盖，legacy 同 rmcp 默认）、`server/discover` 为
  `HubConfig.stateless_list_ttl`（默认 5 秒）+ `private`；`resources/read` 为 `0` + `private`。legacy 线上格式不变。
- 总览：`attach_overview` 对无会话调用方恒为 `None`、`mark_delivered` 不记；`McpSession::discover` 覆盖 rmcp 默认实现，`instructions`
  用 `overview::stateless_instructions`（App 简介同 legacy；说明经 `apps.overview` / `apps.tools` 获取总览、列表不随调用变化）；
  `apps.tools` 对无会话调用方每次在内容最前附带总览（`Overview::render_requested`，不声称"不会重复附带"），`structuredContent.overview`
  同 `apps.overview` 结构。`initialize` 的 `instructions`（`get_info`）不变。
- 测试（`crates/hub/src/mcp.rs`，rmcp `ClientLifecycleMode::Discover`）：`stateless_tools_list_is_pure_function_of_server_state`
  （`stateless_tool_exposure` 为 `All` 与 `Progressive` 各一轮：`apps.tools` / 调用 / `apps.select` / `apps.overview` 前后与另一处理器的
  `tools/list` 线上 JSON 逐字节相同、带 `ttlMs: 5000` / `cacheScope: private`；legacy 渐进暴露照旧变化、无缓存字段；
  `Hub::select_instance` 改变无会话列表）、`stateless_overview_via_discover_and_apps_tools`（discover instructions 含 App 简介、
  调用结果不附带、`apps.tools` 每次附带、资源列表 / 模板列表缓存提示；legacy 首次附带不变）；`overview.rs` 单测
  `render_format`（补 `render_requested`）、`stateless_instructions_text`。T-10：见下方 S6 记录末尾。
- 未做 / 未知：U8（新）——同一 App 多个已连接实例都未聚焦时，`view` 工具取首选实例界面上的，首选实例的"最近活跃"含"最近一次完成调用"
  （`routing.rs`，H20 全局实例状态），调用可能改变列出哪个实例的 `view` 工具，严格说违反 S-F7。处理：保持现状（列出的工具与调用实际
  路由的实例一致更重要），在 spec 3.7 写明例外；S7 前评估无会话列表改按"聚焦 → 连接顺序"取首选实例（需与 4c 页面目录一起定）。
  `apps.tools` 工具描述仍写"加入本会话的工具列表"（只在无会话渐进暴露生效时列出且不准确；结果的 `message` 已按无会话语义）。

**S6 记录（2026-10-02）**

- 主体级 `apps.select`：`AgentTask.selected` 的值改为 `Selection { instance_id, used_at }`；`HubConfig.principal_select_ttl`
  （默认 `DEFAULT_PRINCIPAL_SELECT_TTL` = 60 秒，`0` = 不单独过期）只作用于无会话调用方。路由取用（`selected_for`：工具调用、资源读取、
  `apps.navigate` / `apps.activate`、后台替代判定）时过期即移除、未过期即续期；列出（`apps.list`、`/status`）只过滤不续期。不设定时器
  （过期在取用 / 列出时判定）。结果文本写明作用范围（本机所有无会话客户端）、有效期与"工具列表不变"。U6 默认值 60 秒的理由见
  `DEFAULT_PRINCIPAL_SELECT_TTL` 的 @why（与 `idle_revoke` 30 秒 / 租约上限 60 秒同量级，限制 R1 的串扰时长），仍待真实使用后调整。
- 审批：`ApprovalRequest` 新增 `principal: Option<String>`（MCP 出口恒为 `local`，legacy 与无会话相同——主体来自传输层）与
  `client_name: Option<String>`（rmcp `RequestContext::client_info()`：legacy 取 `initialize`，无会话取请求 `_meta`；仅显示）。
  Hub API 为 `None`，两字段 `skip_serializing_if`，hub-c / hub-node 的审批 JSON 对 Hub API 调用不变；uniffi 的 `ApprovalRequest` 记录
  未加字段（绑定在本次范围外）。
- `/status`：`HubStatus.tasks: Option<Vec<AgentTaskStatus>>`（`id`、`caller`、`kind: CallerKind`、`selections`（含主体级剩余有效期）、
  `leases`（连接 ID + 剩余毫秒）、`inflight`、`idle_ms`），只读快照，先取请求活动再锁任务表（与空闲回收同序）。`CallerKind`、
  `AgentTaskStatus`、`TaskSelectionStatus`、`TaskLeaseStatus` 从 crate 根导出。任务 ID 出现在 `/status`（同用户 / 令牌才能读），
  S8 作为句柄时仍满足 S-F7（对外不可猜测；`/status` 读者本就是同一主体）。`app-mcp-host status` 一行摘要与 doctor 未改（另一会话
  正在改 `crates/host`，且 doctor 不在本次范围）；hub-uniffi / hub-node 的 `HubStatus` 类型未加 `tasks`（JSON 已带）。
- 测试：`principal_selection_ttl_approval_and_status_tasks`（两个实例、默认路由取聚焦实例；无会话 `apps.select` 后连续调用（间隔
  小于有效期）都命中所选实例并续期、列表不变；`/status` 的任务种类 / 选择剩余有效期 / 租约 / 活动、JSON 键；到期后 `apps.list` 不再
  显示、路由回到默认实例，legacy 会话的选择不过期；审批请求的 `principal` / `client_name`，Hub API 不带且 JSON 无这两个键）；
  `task.rs` `selection_idle_ttl_renews_on_use_and_expires`、`caller_keys_are_distinct_and_carry_lifetime`（补 `kind`）。
- **S5 / S6 验证**：`cargo test -p app-mcp-hub -p app-mcp-host` 全部通过（既有测试未改，除 `mcp.rs` 测试辅助函数适配 `Selection`、
  测试连接关闭 rmcp 客户端的列表缓存——rmcp 3.5.0 客户端按 `ttlMs` 缓存列表，`[R] service/client/cache.rs`，不关会让逐字节比较失效）；
  `pnpm --filter e2e test` 19/19 通过（legacy 行为不变）；`cargo clippy --workspace --all-targets` 本次改动 0 警告（唯一警告在另一会话的
  `crates/host/src/activation.rs`）；`cargo clippy -p app-mcp-hub --no-default-features --all-targets` 0 警告（顺带修复
  `tests/call_meta.rs` 的未用导入与 `Hub::shared` 的 dead_code）。T-10：分别把无会话列表改回读任务状态、恢复无会话首次附带并取消选择有效期、
  去掉缓存提示 / 改用 legacy instructions / 去掉 `principal`，对应测试分别失败（1 / 2 / 3 个），恢复后全部通过。

顺序：S3（独立缺陷修复，可立即做）→ S1 → S2 → S4 → S5、S6（可并行）→ S7 → S8（随第 16 项 P1）。
总验收：`cargo test -p app-mcp-hub -p app-mcp-host`、`cargo clippy --workspace --all-targets` 0 警告；默认配置（S7 前）e2e 不变；
S7 后 `e2e/src/mcp-client.ts` 增加 modern 模式，关键用例两代各跑一遍。

## 7. 建议修改的 spec / 文档（由主会话决定，本任务未改）

| 文件 / 节 | 建议 |
|---|---|
| `spec/hub-api.md` 3.6「多会话」 | 改为“两代分述”：legacy 会话；modern 无会话、`CallerKey` = 主体 |
| `spec/hub-api.md` 3.5「租约」 | “会话”改为“调用方键”；modern 无会话结束收回 |
| `spec/hub-api.md` 3.7「渐进暴露」 | 加入 3.3 列表规则（单一定义）；“只向该会话发 list_changed”限定 legacy |
| `spec/hub-api.md` 3.15 键表 | 前缀改 `dev.appwire/`、弃用期、R4 新键、可选 `taskId` |
| `spec/hub-api.md` 3.3 `ApprovalRequest` | `session` 含义两代分述；新增可选 `client_name`（仅显示） |
| `spec/protocol.md` 7.2「首次附带」 | 限定 legacy；modern 经 discover / `apps.tools` / `apps.overview` |
| `spec/protocol.md` 第 4 节错误码分区 | 补“MCP 出口 modern 码映射”与“上游错误不按数值反查”；引用 S-F3 原文要点 |
| `docs/plans/12-mcp-2026-07-28.md` 3.1 / 3.2 / m10 | 指向本文件 3.6 差异表；m10 表述作废 |
| `docs/plans/16-agent-os.md` P2、U8 | P2：主体级 `hide` 可行（3.3）；U8：任务 ID 以工具参数句柄为主、`_meta` 为可选通道（3.4） |
| `docs/plans/19-result-contract.md` U3 / R4，`docs/plans/18-user-loop.md` U2 | 标为已回答，指向本文件第 4、5 节 |

## 8. A-05 三清单

### 8.1 事实

见第 1 节 H1–H20（代码）与 2.1 节 S-F1–S-F10（规范与 rmcp）。补充：`git tag` 为空（无发布版本）；`app-mcp/*` 键只出现在 `crates/hub/src`
（`names.rs`、`call.rs`、`types.rs`、`request_meta.rs`）与 `crates/hub/tests/{agent_control,safety}.rs`；git 历史起点为 2026-10-01 导入（`711bf77`）。

### 8.2 未知

| # | 未知 | 处理 |
|---|---|---|
| U1 | 项目是否拥有 `appmcp.dev`（或 `appwire.dev`）域名；机主倾向工作名还是品牌名前缀 | **已定**（机主 2026-10-02）：品牌名前缀 `dev.appwire/`；S1 已实施 |
| U2 | 通用客户端（Claude Code 等）是否透传 / 允许设置厂商 `_meta` 键；工具结果 `_meta` 是否对模型可见 | **验证**：临时 Host（`--home` 临时目录）+ Claude Code 实测，记录请求 `_meta` 全部键；结论只影响“可选通道”是否有用，不影响正确性 |
| U3 | modern 客户端是否允许调用 `tools/list` 中未列出的工具（决定渐进暴露在 modern 下能否保留“按全名调用”） | **待验证**（S5 时无法实测：Hub 在 S7 前只协商到 2025-11-25，Claude Code 回退 legacy）。已按保守方案实施：`HubConfig.stateless_tool_exposure` 默认 `All`，渐进暴露默认只在 legacy 生效；S7 放开 2026-07-28 后用 Claude Code 实测，允许时再考虑改默认 |
| U4 | rmcp 自定义 `_meta` 键在 modern 路径是否原样到达 handler | **推断**可以（S-F10 透明 map）；S1 用 `ClientLifecycleMode::Discover` 的集成测试断言。S4 已验证无会话请求经 Discover 生命周期到达 handler（`mcp.rs` 测试），自定义键尚未单独断言 |
| U5 | -32001…-32015 是否早于 2026-07-28 分配（git 历史从 2026-10-01 起，无法判定） | 不影响结论：modern 出口不再发这些码（第 5 节） |
| U6 | 主体级 `apps.select` 的空闲 TTL 默认值 | **保守，已实施**：`principal_select_ttl` 默认 60 秒（与 `idle_revoke` 30 秒、租约上限 60 秒同量级）、可配置、取用即续期；S7 后按真实使用观察再定 |
| U8 | 多实例都未聚焦时，`view` 工具按首选实例列出，首选实例的"最近活跃"含最近完成的调用（H20），调用可能改变无会话列表 | **保持现状、已写入 spec 3.7 例外**；S7 前与 4c 页面目录一起评估无会话列表改按"聚焦 → 连接顺序"取首选实例 |
| U9 | `stateless_list_ttl` 默认 5 秒是否合适（12 迁移计划 U5） | S7 实现 `subscriptions/listen` 后用 Claude Code 观察是否按 TTL 重拉，再调 |
| U7 | 规范本身后续是否给“本地错误”“需用户操作”分配标准码 | 跟踪 changelog；有标准码时在 MCP 出口映射，AppWire 协议不变 |

### 8.3 风险（涉及兼容性、并发、权限）

| # | 风险 | 收敛措施 |
|---|---|---|
| R1 | N5 之前所有 modern Agent 共用一个主体：一个 Agent 的 `apps.select` / `apps.release` 影响另一个 | 结果文本写明作用范围；`apps.select` 带 TTL；S8 任务句柄提供隔离；`/status` 显示主体级选择 |
| R2 | modern 取消渐进暴露的动态列表后工具数过多 | 全局选择与策略 `hide` 仍可裁剪；`ttlMs` 让客户端缓存；U3 实测后再定默认 |
| R3 | 改 `_meta` 前缀破坏已有 Agent 配置 | 请求侧旧键弃用期兼容；冲突显式失败 |
| R4 | 修复上游错误反查后，依赖旧（错误）分类的调用方行为改变 | 原码保留在 `details.upstreamCode`；在报告与 spec 中写明 |
| R5 | `CallerKey` 收敛与 4c、16 P1 同时改 `hub.rs` 冲突 | S4 与 16 P1 同批由一个会话做；S3 / S1 先行且只改小范围文件。**已按此执行（2026-10-02）** |
| R6 | 规范与 rmcp 后续小版本调整 dual-era 细节 | 回退开关；`cargo update` 后跑 S4–S7 的协议测试 |
