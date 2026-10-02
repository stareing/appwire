# 16 Agent OS：数据、权限、事件、事务与协同（计划）

> 状态：计划（2026-10-02）。把 AppWire 视为 Agent 的"系统调用层"（模型 = 用户态程序，App 工具 = 系统调用，Hub = 内核），
> 按操作系统子系统找差距。行为契约分别落在 `spec/protocol.md`、`spec/hub-api.md`、`spec/manifest.md`；本文件只负责分析与任务拆分，
> 实施结果写入 `TASKS.md` 第 16 项。与第 13–15 项已规划内容不重复。
> 2026-10-02 补充：新增第零部分"进程模型"（P1–P6）；内核范围（做什么、不做什么）以 `CLAUDE.md`「微内核范围」为准，本文件只引用。

## 0. 子系统对照

| 子系统 | 已有 / 已规划 | 本项补的 |
|---|---|---|
| 进程管理（App 侧） | 生命周期、休眠唤醒、4e 功耗（B2 自适应租约为可替换的缺省策略）、4d 连接即唤醒 | Agent 显式 `apps.activate` / `apps.release`（4f a，替代原 O5 预测预热） |
| 进程管理（Agent 侧） | 无：身份、句柄、租约、订阅各自挂在 MCP 会话上 | P1 Agent 任务对象；P5 调用对象、状态查询与后台作业控制 |
| 资源记账（cgroups） | 按 App / 工具限流（第 14 项 S3）、按 App 唤醒上限 | P3 按 Agent 记账与配额 |
| 进程间通信 | IPC / WebSocket、按名寻址（4d） | N1 数据句柄（App 间传数据不经模型） |
| 命名与发现 | 4d 多来源发现 | N4 标准意图（按动作找 App） |
| 权限 | 确认与授权归 Agent / App（第 14 项第 1 节）；本库只如实传递声明、限流 | N5 Agent 身份（只用于句柄绑定与调用日志）；N2 句柄访问范围 |
| 调度 | 按实例路由、`apps.select` | N6 多 Agent / 人机并发仲裁；P6 交互优先的 QoS 与 Agent 截止时间 |
| 中断与事件 | 资源订阅 | N3 事件与触发器；P4 持久信箱 |
| 安全执行点 | 仅嵌入式 Hub 有 `ApprovalHandler`；常驻 Host 无 | P2 策略挂点（2026-10-02 决定实施，含第 18 项 L5） |
| 事务 | 无 | N7 预演、幂等键、跨 App 补偿 |
| 上下文（内存） | 渐进暴露、4c 界面级暴露 | O1 工具检索与排序；O3 只读结果缓存 |
| 驱动 | 第 15 项导入器、OS 层 | — |
| 包管理 | 第 13 项分发 | O4 工具 schema 演进与弃用 |
| 可观测 | doctor / status、第 11 项 tracing | 端到端链路（Agent 一轮 → 调用 → App handler）并入第 11 项；P7 Hub 自身状态作为资源（/proc） |

## 1. 已知的已知（代码可证的事实）

| # | 事实 | 证据 |
|---|---|---|
| K1 | 协议无进度通知，长调用只能等到 `response_timeout` | `spec/protocol.md` 无 progress 消息；全仓无 `notifications/progress` 实现 |
| K2 | 无调用级幂等键 / 去重；`idempot` 只出现在句柄 `dispose` / `release` 语义中 | `crates/native/src/lib.rs` 334、377、439 行 |
| K3 | 无预演（dry-run / preview）、无工具检索、无信息流标签、无事件触发器 | 全仓搜索 `preview` / `embedding` / `taint` / `trigger` 均无对应实现（`crates/hub/src/lifecycle.rs` 的 trigger 为唤醒触发，非事件） |
| K4 | Hub 只知道 MCP 会话与 `clientInfo`，不区分 Agent 身份与授权 | `crates/hub/src/call.rs` `ApprovalRequest::session` |
| K5 | 资源订阅与 `realtime` 声明已有（4e B3）；`toolsHash` 可判断工具列表变化 | `spec/lifecycle.md` 第 13 节；`crates/protocol/src/hash.rs` |
| K6 | codegen 已把工具映射到 App Intents / AppFunctions / Windows App Actions / 鸿蒙意图 | `crates/codegen/src/targets` |
| K7 | 渐进暴露按 App 分层（`apps.*` + `apps.tools(appId)`） | `TASKS.md` 第 2 项；`spec/hub-api.md` 3.7 |
| K8 | rmcp 3.5.0 已支持 MCP 进度通知与 elicitation（服务端 `Peer`） | `~/.cargo/registry/.../rmcp-3.5.0/src/service/server.rs`：`notify_progress`（914 行）、`elicit`（1144 行） |
| K9 | 会话状态 `SessionState { selected, delivered, leases, exposed }` 按 MCP 会话保存，会话 `Drop` 时清理。**2026-10-02 起**迁入 `AgentTask`（P1），按调用方键保存 | `crates/hub/src/task.rs`；`docs/plans/12-mcp-2026-07-28.md` L3 / L5 |
| K10 | MCP 2026-07-28 删除协议级会话（SEP-2567），跨调用状态改用服务器签发的显式句柄 | `docs/plans/12-mcp-2026-07-28.md` M1、F1 |
| K11 | `ApprovalHandler` 只在 Hub SDK 配置中（`crates/hub/src/types.rs:532`），`crates/host` 未使用 | `grep approval crates/host/src` 无结果 |
| K12 | 唤醒上限只按 App 计（`HubConfig::wake_rate_limit`，默认每 App 每分钟） | `crates/hub/src/hub.rs:144, 166` |
| K13 | MCP 资源列表只含 App 资源与上游资源，无 Hub 自身状态 | `crates/hub/src/mcp.rs:122-152` `list_resources` |

## 2. 已知的未知

| # | 未知 | 处理 |
|---|---|---|
| U1 | 断线 / 唤醒重连后，进行中的写调用是否会被重发（决定 N7 幂等的紧迫度） | **先验证**：读 `crates/core` 调用队列与 `crates/hub` 派发重试路径，写复现测试；结论写回本文件 |
| U2 | MCP 侧传句柄的载体（`resource_link` 内容 + Hub 自有 URI 方案）与各 Agent 的呈现 | **验证**：以 MCP 2025-11-25 规范与 Claude Code 实测为准；第 12 项无会话协议下句柄的生命周期一并设计 |
| U3 | MCP Apps（UI 资源扩展）的规范现状与 Agent 支持面 | **验证**：查官方规范与 Claude Code 支持情况；未确认前不实施 N8 |
| U4 | 标准意图词表的来源（自定义 vs 对齐 schema.org Actions / App Intents 领域 / AppFunctions 预置 schema） | **调研**后定词表；先做 5 个高频动词试点 |
| U5 | 本地向量检索的体积与依赖（移动端 `mobile` 精简包能否承受） | **保守**：默认关键词 + 使用统计排序；向量检索为可选特性（cargo feature），默认关闭 |
| U6 | 信息流控制的标签传播粒度（整次结果 vs 字段级）与误伤率 | **保守**：先做结果级标签 + 只拦"私密 → 外发"一类；记录拦截次数后再细化 |
| U7 | 人机并发的"用户正在操作"信号来源（各 UI 框架焦点 / 编辑状态） | SDK 侧显式 API（`busy()` / 对象锁），不自动推断 |
| U8 | 任务 ID 在 modern 请求中的载体（`_meta` 字段名）与 Agent 是否会回传 | **核实** MCP 2026-07-28 规范 `_meta` 约定与第 12 项第 3 节设计；Agent 不回传时退化为"每请求一个短命任务" 。**更新（2026-10-02）**：按第 12 项 3.4，任务 ID 以工具参数句柄为主、`_meta` 为可选通道（S8）；都没有时退化为调用方键的默认任务（`principal:<主体>`，已实施），而不是每请求一个任务。**S8 已实施（2026-10-03）**：参数 `taskId` + `_meta` `dev.appwire/taskId`；Agent 是否回传参数句柄未经 Claude Code 实测 |
| U9 | 任务对象的租约时长与心跳来源（Agent 无显式心跳时以请求流活跃为准） | **保守**：复用 4e 自适应租约的"请求流空闲"判定（`spec/hub-api.md:370`），默认值可配置。**已按此实施**：`HubConfig.task_idle_ttl`（默认 10 分钟，`0` 关闭），只作用于无会话调用方的任务 |
| U10 | 持久信箱在 Agent 侧的取件方式（下次请求附带 / `subscriptions/listen` / 专用资源） | **验证** Claude Code 对资源更新通知与 `subscriptions/listen` 的支持后定 |
| U11 | P2 策略挂点是否与第 14 项职责划分冲突 | **已决定（2026-10-02）**：实施。挂点只提供执行位置，规则由用户 / 厂商写，默认无规则、行为与现状一致；本库不内置任何判断，与第 14 项不冲突 |
| U12 | 规则文件格式与热加载（文件监视 vs `reload` 命令）及与 `config.json` 的关系 | **已实施（fe622b9）**：独立 `<home>/policy.json`；启动时不合法 → Host 拒绝启动（无旧规则可保留，静默全放行违背用户意图）；`reload` 时不合法 → 保留旧规则并报错；`doctor` 检查 |

## 3. 未知的已知（可复用）

- 唤醒、租约、合并窗口（4e）可直接承载 N3 触发器：事件到达 → 按规则唤醒 Agent 侧回调，不需新传输。
- 资源订阅 + `realtime`（4e B3）是事件的现成通道；N3 只需加"事件"语义与规则表。
- App 的注解与内容标注由第 14 项如实传递，Agent 据此自行决定确认；本项不新增确认通道。
- 第 15 项 X2 `undo` 是 N7 跨 App 补偿的原语；第 11 项调用日志是补偿的依据。
- codegen 的系统意图映射（K6）是 N4 标准意图的输出端。
- `toolsHash` 与快速恢复（K5）是 O4 schema 演进的检测基础。

## 4. 未知的未知（限制手段）

- **默认关闭或保守默认**：N1 句柄有 TTL 与大小上限；N3 触发器每条规则有频率上限并经第 14 项限流；O1 向量检索默认关；N9 远程默认关。
- **单一定义**：意图词表、数据标签、句柄 URI 格式各自只在一处定义（spec），其他位置引用。
- **回归测试**：每项附确定性测试（句柄过期 / 越权访问、标签传播、触发器风暴、重复写调用去重、并发排队公平性）。
- **可观测**：`/status`、`doctor` 增加句柄数量与内存、拦截次数、触发器触发次数、去重命中数。

## 5. 方案与任务

### 第零部分：进程模型（地基；与第 12 项双版本改造同期）

- **P1 Agent 任务对象**：Hub 签发任务 ID，寿命由租约维持（U9），不依赖传输连接；名下持有句柄（N1 / 第 17 项）、对象锁（N6）、唤醒租约（4e）、
  订阅、配额计数（P3）与调用日志归属（第 11 项）；过期由 Hub 统一回收。legacy MCP 会话 = 一会话一任务；modern 无状态请求经任务句柄（U8；S8 实施为工具参数 `taskId` 为主、`_meta` 可选）。
  N5 Agent 身份记在任务对象上，不另立。`SessionState`（K9）迁入任务对象。
  - **最小任务对象已实施（2026-10-02，与第 12 项 S4 同批）**：`crates/hub/src/task.rs` `AgentTask`（Hub 签发 `task-<128 位十六进制>` ID；
    名下 `apps.select` 选择、已附带总览、租约、渐进暴露）按调用方键寻址：legacy MCP 会话 / Hub API 会话一会话一任务（随结束信号回收），
    无会话 MCP 请求按主体一任务（`principal:local`），寿命按请求流空闲（U9：复用租约的请求活动记录，`HubConfig.task_idle_ttl` 默认 10 分钟），
    回收时收回其租约。契约见 `spec/hub-api.md` 3.6「调用方与 Agent 任务」。
  - **第 12 项 S5 / S6 补充（2026-10-02）**：无会话任务上的展开记录与选择不再影响 `tools/list`（S5）；主体级 `apps.select` 选择带空闲有效期
    （`HubConfig.principal_select_ttl` 默认 60 秒，取用即续期）；任务以只读形式出现在 `/status` 的 `tasks`（`id`、调用方键与种类、
    选择、租约、进行中请求、空闲毫秒）；`ApprovalRequest` 带 `principal` 与仅供显示的 `client_name`。见 `spec/hub-api.md` 3.3 / 3.6 / 3.7 / 3.9。
  - **第 12 项 S8 任务句柄（2026-10-03）**：任务 ID 作为显式句柄对外——`apps.task.begin` 为无会话主体签发（每主体至多
    `HubConfig.max_task_handles`，默认 32），参数 `taskId`（`apps.list` / `select` / `navigate` / `activate` / `release` / `apps.task.end`）
    为主通道，`_meta` `dev.appwire/taskId` 为可选通道（任何工具调用）；各句柄的选择与租约互不影响，寿命复用 `task_idle_ttl`（U9），
    过期 / 结束后出示 → `INVALID_INPUT`（`reason: task-expired`，可恢复：重新 `apps.task.begin`）；legacy 会话与 Hub API 出示 →
    `INVALID_INPUT`（`task-handle-unsupported`）。契约见 `spec/hub-api.md` 3.6「任务句柄」，记录见第 12 项 S8 记录。
  - 未实施（挂点已留在 `AgentTask` 上）：P2 按 Agent 匹配、P3 记账、N6 锁、第 17 项句柄与订阅归属；通用客户端（不填 `_meta`）
    的 App 工具调用仍按主体默认任务路由。N5 身份已实施（2026-10-03，见第三部分 N5 记录）。
- **P2 策略挂点（2026-10-02 决定实施；第 18 项 L5 暴露开关由此实现）**：类比 LSM，本库只提供执行点，不内置任何判断；无规则时行为与现状完全一致。
  - **执行点**：列出（`tools/list`、`apps.*`）、调用、唤醒、句柄访问（句柄挂点只定义类型，规则用到即校验失败，待第 17 项句柄落地）。
  - **两种动作**：`hide`（不出现在任何列表中，调用按 `TOOL_NOT_FOUND`）与 `deny`（可见但调用被拒）。
    `hide` **只能全局生效、不能按 Agent 区分**：MCP 2026-07-28 要求列表不得按连接变化（第 12 项 M1）；按 Agent 区分的规则只能用 `deny`。
  - **匹配条件**：App、工具名（支持 `*` 后缀通配）、Agent 身份（P1 / N5）、App 声明的 MCP 注解（如 `destructiveHint`）。
    按注解匹配是**用户写的规则引用 App 的声明**，本库不推断风险、不改写声明；工具未声明的提示**不匹配**（不按 MCP 缺省值推断）。
    按 Agent 匹配等 P1，当前未实施。
  - **规则来源**：Hub 本身不读文件，规则一律经 `set_policy` 设置。常驻 Host 由 `<home>/policy.json` 提供（U12）：
    `app-mcp-host policy show / validate / reload / hide / deny [--wake] / remove`，`reload` 由命令行读文件后 `POST /policy` 交给运行中的 Host（鉴权同 `/status`）；托盘 X3 为后续入口。
    嵌入式 Hub 由厂商调用 `set_policy`，现有 `ApprovalHandler`（K11）保留为调用执行点上的回调形态（E-06）。
    Host 不支持外部程序回调（避免执行任意进程），需要时由厂商嵌入 Hub 实现。
  - **拒绝结果**：新错误类别 `POLICY_DENIED`（-32018；附命中规则的标识，不附规则内容），定义在 `spec/protocol.md` 第 4 节；与 `USER_REJECTED`（用户当场拒绝）区分。
    MCP 错误文本保持 `KIND: message`，规则标识等只放在 `structuredContent.error.details`。
  - **可观测**：`doctor` 列出生效规则与每条的命中次数；命中计数在 `reload` 时清零，`hide` 过滤列表不计命中；命中记入第 11 项调用日志。
  - **已知局限**：Hub 不改写 App 总览文本，被隐藏的工具仍可能在总览中被提到（调用仍按 `TOOL_NOT_FOUND`）。
  - **后续执行点**：4e B2 自适应租约定位为可替换的缺省策略（`--fixed-lease` 可关；Agent 显式 `apps.release` 优先），后续可经本挂点替换（4f）。
  - **实施（fe622b9，2026-10-02）**：按 App / 工具 / 注解匹配、`hide` / `deny` 已完成。
  - **按 Agent 匹配（2026-10-03，随 N5）**：规则字段 `agent`（Agent 名模式，只用于 `deny`；`hide` + `agent` 校验报错），调用 / App 级调用 /
    唤醒执行点按调用方的 Agent（`CallCtx::agent`、资源读取按调用方键）匹配，本机主体与 Hub API 不匹配；Host `policy deny --agent`，
    默认 id `deny-<app>-<tool>-for-<agent>`；各绑定的规则类型加 `agent`。测试：hub `policy.rs deny_by_agent`、`tests/agents.rs
    deny_rule_applies_only_to_named_agent`（让规则忽略 `agent` 时两者都失败）、Host `agent_registry_cli_and_tokens`、Python、C#。
    未验证：Kotlin / Swift 封装（只靠生成的记录带缺省值，未跑其测试）。
- **P3 按 Agent 记账与配额**：调用次数、唤醒次数、传输字节按任务对象 / Agent 身份累计，可配置上限，超限返回 `RATE_LIMITED`（与第 14 项 S3 同一错误码）；
  `status` / `doctor` 能回答"谁唤醒了这个 App 多少次"。
  - **实施（2026-10-03）**：契约见 `spec/hub-api.md` 3.11「按调用方记账」与 `limits.agent_rate`。
    - 事实：记账按 Agent 身份（`CallerKey::usage_subject`：`agent:<名>` / `local` / `api`），不按任务对象——任务会过期回收，按 Agent 累计才能回答
      "谁唤醒了多少次"；记账点为调用守卫（`guard_call`：调用、参数字节、被限流）、结果接收（App 结果 / 错误、上游结果字节）与唤醒准入
      （`admit_wake` = 唤醒策略 + 唤醒计数，取代分散的策略检查调用）。配额为限流的第三级（每个已登记 Agent，跨所有 App），默认不限。
    - 测试：`usage.rs` 单元（含两张表的上限）、`limits.rs agent_level_limits_across_apps`、`tests/agents.rs agent_quota_and_usage_accounting`
      （配额只拦 claude、各主体计数与字节）、`tests/lifecycle.rs` 唤醒计数（Hub API 主体两次唤醒）；Host doctor「调用方用量」单元与
      `serve.rs agent_registry_cli_and_tokens`；`@app-mcp/hub` 38、C# Hub 25、Python 16、hub-uniffi 30。
    - 未知 / 未做：计数不持久化（Host 重启清零）；按 Agent 的唤醒次数上限（只有调用频率上限，唤醒由第 4e 项按 App 的唤醒速率限制）；
      传输字节不含 MCP 出口对 Agent 的序列化开销；Kotlin / Swift 封装未跑测试。
    - 风险：每次调用都序列化一次参数计字节（之前只在设置了参数上限时序列化，默认即设置，开销不变）。
- **P4 持久信箱**：N3 事件到达而无存活任务时进入信箱（TTL、条数上限），下次接触时投递（U10）。
- **P5 调用对象与后台作业控制**：每次调用是一个可查询的对象，状态为 `pending / running / completed / failed / cancelled / timeout`（2026-10-02 由 4f b 并入）；调用可转为脱离请求的作业，支持列出 / 取消 / 等待 / 重新挂接；与 O2 共用取消路径，载体对齐 MCP tasks 扩展（第 12 项 M6）；第 19 项 R1 的 `pending` 结果可引用调用对象。
  - **调用状态机**（4f i）：`CREATED → ACTIVATING → RUNNING → 结果`，写入 `spec/protocol.md`；平台相关状态（前台 / 后台 / 挂起）只作诊断字段 `platform_state`（`status` / `doctor`），不进核心状态。
  - **作业状态归属**（4f h）：脱离请求的作业状态由 App 持久化，Hub 只转发作业 ID 与状态查询、不在内存保存作业状态（App 进程被系统回收后状态不丢）；进行中调用的状态（状态机）由 Hub 持有，调用结束即释放。
- **P6 交互优先 QoS**：调用可带优先级与截止时间；截止时间由 Agent 在 MCP 请求 `_meta` 中以相对毫秒 `dev.appwire/timeoutMs` 给出，Hub 取其与 `response_timeout` 的较小者、只限制等待 App 结果（4f c，已实施 7f587d8；键名随第 19 项 R4）；用户在场的交互调用优先于后台作业，冲突时后台排队或让路。
- **P7 Hub 自身状态作为资源**：已连接 App、任务、句柄、配额余量以只读 MCP 资源暴露（K13），Agent 用 `read` 自查。

N6 对象锁随 P1 改为租约：持有任务过期即释放（健壮锁），否则崩溃的 Agent 会永久锁住 App。

### 第一部分：正确性（最先做）

- **N7a 幂等键**：写及以上风险的调用带 `callId`（Hub 生成，重发保持不变），SDK 在有效期内按 `callId` 去重并返回首次结果；先完成 U1 验证。
  `callId` 只覆盖 Hub ↔ App 一段的重发；Agent 侧的 MCP 重试由 Agent 幂等键补齐（4f j）：MCP `_meta` 的 `dev.appwire/idempotencyKey`（1–256 字符）原样传入 `ToolsInvokeParams.idempotencyKey`，handler 上下文可读（已实施 7f587d8），如何去重由 App 决定（键名 `dev.appwire/` 前缀，第 12 项 S1）。
- **O2 进度与取消**：协议新增进度消息，Hub 透传为 MCP `notifications/progress`；取消一路传到 App handler（已有取消路径则复用）。

### 第二部分：数据面与信息流

- **N1 数据句柄**（由第 17 项第二部分实现，契约见 `docs/plans/17-content-files.md`，此处不另行定义）：工具结果可返回句柄（`resource_link` + TTL + 大小上限），其他工具参数可引用句柄，Hub 在 App 间搬运，模型只见元信息；
  句柄绑定 MCP 会话与授权范围，过期即删。
- **N2 句柄访问范围**：句柄只能被签发它的会话使用、有 TTL，App 可声明句柄只读或仅限指定 App 消费；本库只执行 App 的声明，不做数据分级或外发判定（第 14 项第 1 节）。

### 第三部分：多 Agent 与协同

- **N5 Agent 身份**：Agent 首次连接时登记身份（`clientInfo` + 本机令牌 / 进程信息），记在 P1 任务对象上，用于句柄绑定、P3 记账与调用日志；授权由 Agent 自身配置负责，本库不做。
  - **实施（2026-10-03）**：按 Agent 发令牌（不用 `clientInfo`：自报不可信，第 12 项 S-F6）。契约见 `spec/hub-api.md` 3.6「Agent 身份」。
    - 事实：`HubConfig.agents: AgentsConfig`（`crates/hub/src/agents.rs`，名字 / 令牌校验、常量时间核对）、`Hub::set_agents`、`POST /agents`；
      `/mcp` 核对令牌时得出 `Principal::Agent(名)`，经 HTTP 请求扩展交给 `McpSession`（rmcp 3.5.0 把 `http::request::Parts` 放进请求上下文，
      `tower.rs` 1524 / 2072）；调用方键 `principal:agent:<名>`，legacy 会话在 `initialize` 时记下身份（`CallerKey.agent`，相等与哈希只看键字符串）。
      任务、句柄（归签发 Agent）、主体级选择、租约、句柄与 listen 流上限、审批 `principal` 随之按 Agent 分开；`/status` `agents`（只列名字）
      与 `tasks[].agent`。Host `<home>/agents.json`（0600）+ `app-mcp-host agent add / remove / token / list / reload`，doctor「Agent 登记」。
    - 测试：`crates/hub/tests/agents.rs` 5 个（两个 Agent 令牌的任务与句柄归属、句柄上限按 Agent、legacy 会话记身份、`/agents` 替换与
      Agent 令牌不能读 `/status`、启动校验；把主体恒置为本机时其中 3 个失败）；`agents.rs` / `task.rs` / `http_server.rs` 单元；Host
      `serve.rs agent_registry_cli_and_tokens`、`agents.rs` 与 doctor 单元。
    - 未知 / 未做：stdio 与 `serve_mcp_stream`（含移动端 Binder 上的 MCP）没有 HTTP 头，恒为本机主体，嵌入式 Hub 的调用方识别随第 4g e 项；
      `setup` 写入 Agent 配置时尚不为每个 Agent 自动登记令牌；hub-c / hub-node / hub-uniffi / C# 尚未暴露 `agents`；IPC 上出示 Agent 令牌
      只是身份声明（同一用户本来能读 `agents.json`），不构成隔离。
    - 风险：令牌泄露即可冒用该 Agent 的身份——只影响区分与归属，不扩大权限（Agent 令牌不能访问 `/status`、`/policy`、`/agents`）；
      文件以 0600 写入，doctor 检出权限过宽。
- **N6 并发仲裁**：SDK 提供 `busy()` / 对象锁；Hub 对写调用排队或返回明确错误；多会话对同一 App 公平排队。
  工具可声明 `concurrency: N` / `exclusive`（同一资源互斥），SDK 按声明排队，队列上限可配置，满时返回明确错误（4f k，由 4f 实施）。

### 第四部分：场景扩展

- **N3 事件与触发器**：App 在清单声明可发出的事件；用户 / Agent 注册"事件 → 提示"规则，Hub 在事件到达时回调 Agent 宿主（Hub SDK 回调；Host 侧经 MCP 通知）；本库只投递事件，不代 Agent 发起调用（调用及其授权由 Agent 负责）。
- **N4 标准意图**：定义通用动词 schema（先试点 `message.send`、`calendar.create`、`media.play`、`file.share`、`navigation.open`），App 声明实现；
  Hub 按用户默认 App 路由；codegen 输出到系统意图框架。
- **O1 工具检索**：`apps.search(query)`，按关键词、最近使用、成功率、当前可见界面（4c）排序；可选本地向量索引（U5）。
- **O3 只读结果缓存**：`read` 工具与资源按 App 声明的 TTL / 版本号缓存，命中时不唤醒 App。
- **O4 schema 演进**：字段弃用标记、兼容规则与 Agent 侧缓存失效策略，写入 `spec/manifest.md`。
- ~~O5 冷启动预算与预测预热~~：**已删除（2026-10-02，机主同意，见 `TASKS.md` 4f）**——预测预热属于策略（`CLAUDE.md`「微内核范围」）；改为 Agent 显式调用的内置工具 `apps.activate(appId)`（只唤醒不调用）/ `apps.release(appId)`（收回本会话在该 App 的租约），由 4f 实施。

### 第五部分：依赖外部规范 / 远程（最后）

- **N7b 预演与跨 App 补偿**：工具可选 `preview`（返回将执行的操作，供 Agent 展示）；多步操作失败时按 `undo` 逆序补偿（依赖第 15 项 X2）。
- **N8 App 界面嵌入 Agent**：U3 确认后再定。
- **N9 跨设备接力**：依赖第 15 项 R1 远程鉴权设计。
- **N10 本地小模型辅助**：工具排序、参数提示、敏感信息识别；可选、默认关闭。按 `CLAUDE.md`「微内核范围」属于用户态服务，实施前重新评估是否留在本库。

## 6. 顺序与验收

0. 第零部分：P1 与第 12 项双版本改造同期（两者都重写会话状态）；P3、N6 健壮锁随 P1；P2 在 P1 之后（按 Agent 匹配依赖任务对象，按 App / 工具 / 注解的规则可先行）；P4 随 N3；P5 随 O2；P6、P7 在第四部分之前。
1. 第一部分（N7a、O2）与第 14 项同期完成。
2. 第二部分（N1 → N2）在 4c 之后；第三部分（N5、N6）在 4d 之后（Agent 身份与按名寻址共用身份模型）。
3. 第四部分与第 15 项穿插；第五部分最后。

验收：
- 断线重连后重复的写调用只执行一次（回归测试复现 U1 场景）。
- 长任务在 Claude Code 中显示进度，取消后 App handler 收到取消。
- "把截图发给联系人"全程图片不进入模型上下文；句柄被其他会话或未声明的 App 使用时被拒绝。
- 用户编辑中的对象不被 Agent 覆盖；持锁 Agent 崩溃后锁在租约到期时释放。
- 经 P2 规则：全局 `hide` 的 App / 工具不出现在任何 Agent 的列表中；按 Agent 的 `deny` 规则使该 Agent 调用返回 `POLICY_DENIED`，其他 Agent 不受影响；无规则时全部现有测试不变。
- 任务对象过期后其句柄、锁、租约、订阅全部回收（回归测试）；`doctor` 按 Agent 显示唤醒与调用计数。
- 事件触发器、标准意图、工具检索各有端到端示例。
