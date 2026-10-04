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
  - 未实施（挂点已留在 `AgentTask` 上）：第 17 项句柄与订阅归属（P2 按 Agent 匹配、P3 记账、N6 锁已实施，见各自条目）；通用客户端（不填 `_meta`）
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
  - **实施（2026-10-03，第一阶段：调用对象）**：契约见 spec/hub-api.md 3.6「调用对象」；`crates/hub/src/call_objects.rs`。
    - 事实：`HubShared.calls`（原 callId → 取消信号）扩为调用对象 `CallEntry`（调用方键、工具全名、阶段、开始时刻、实例、连接、最近进度），
      登记与释放仍在 `dispatch.rs call()`（序号防覆盖）。阶段：`created` → `approving`（`approve` 真正询问用户时）→ `activating`
      （唤醒 / 导航前）→ `running`（发 `tools/invoke` 前、调上游前、执行内置工具前）。进度：`route_progress` 先记到调用对象（只认执行
      连接，不论调用方是否请求进度通知）。诊断 `platformState` = 执行实例最近上报的可见性（4f i：不进阶段）。内置工具 `apps.calls`
      （只读）/ `apps.cancel`（按调用方键 + 任务句柄判定归属，他人的与不存在的回复相同）总是列出，属任务级工具（可带 `taskId`）。
      `HubStatus.calls`（含调用方键）、`apps/self` `calls`、Host 摘要与 doctor（最多 5 个）。
    - 测试：`tests/call_objects.rs`（运行中带实例、进度与 platformState；apps.calls 只见自己且不含本次查询与调用方键；取消他人 / 不存在
      → TOOL_NOT_FOUND；取消自己 → 发起方 CANCELLED、对象释放；审批中为 approving）；Host `callers_text_lists_calls`；hub-uniffi 转换。
      变异：去掉归属检查、不置 running、不记进度分别被检出。
    - **第二阶段决定（2026-10-03，机主选 a）**：Hub 不做脱离请求 / 重新挂接，长作业一律由 App 以 `pending` + `stateResource` 持久化；
      Hub 的调用对象只管进行中的调用。下面的选项保留作决策记录。
    - 未知 / 未做（原第二阶段选项）：**脱离请求与重新挂接**（Agent 不等结果、稍后 `wait` 取结果）要求 Hub 在调用结束后保留结果，
      与"调用结束即释放"冲突——可选：(a) 不做，长作业一律走 App 的 `pending` + `stateResource`（现状，作业状态在 App、进程回收不丢）；
      (b) Hub 有界保留已结束的脱离调用结果（TTL、条数与字节上限、只给发起方）；(c) 对齐 MCP tasks 扩展（第 12 项 M6，规范仍是实验性）。
      另：调用对象不持久化（Hub 重启即无，进行中的调用本也随之失败）；`apps.calls` 不列出其他 Agent 的调用（机主看 `/status`）。
    - 风险：内置工具多两个（列表变长约 2 项描述）；每次 `/status` 为每个调用查一次注册表（只在读状态时）。
- **P6 交互优先 QoS**：调用可带优先级与截止时间；截止时间由 Agent 在 MCP 请求 `_meta` 中以相对毫秒 `dev.appwire/timeoutMs` 给出，Hub 取其与 `response_timeout` 的较小者、只限制等待 App 结果（4f c，已实施 7f587d8；键名随第 19 项 R4）；用户在场的交互调用优先于后台作业，冲突时后台排队或让路。
  - **实施（2026-10-03，调用优先级）**：契约见 spec/protocol.md 5.3「优先级」、spec/hub-api.md 3.15「调用优先级」。
    - 事实：Agent 在 `tools/call` 请求 `_meta` 给 `dev.appwire/priority`（`interactive` / `normal` / `background`，Hub 严格校验，
      不合法 → `INVALID_INPUT`）；Hub API `CallRequest.priority`。Hub 只转交为 `ToolsInvokeParams.priority`（normal 不写出），不按优先级
      排队、限流或唤醒（Hub 不排队；资源保护对所有优先级相同）。SDK 核心调用队列按（优先级, 到达顺序）插入（`calls.rs enqueue`），
      4f k 的调度规则在此顺序上进行；队列满时若有更低优先级的排队调用，拒绝其中最后到达的一个（`RATE_LIMITED` `data.preempted`），
      否则拒绝新调用。SDK 宽松解析（不认识的取值 = normal，新旧版本互不拒绝）。各 App SDK 无需改动（核心统一调度）。
    - 测试：协议宽松解析与省略；核心 `enqueue` 单元、`queued_calls_start_by_priority`、`full_queue_preempts_lower_priority`；Hub
      `request_meta` 校验、`agent_control priority_reaches_app_queue`（Hub API → 真实 native App 的开始顺序）与 MCP `_meta` 不合法值；
      hub-c v21 JSON、hub-uniffi 转换；一致性用例 `call-priority`（含不认识的取值）11 个 runner 全 pass。变异：插入不按优先级、
      不让路、Hub 不转交分别被检出。
    - 未知 / 未做：已开始的后台调用不被抢占（handler 无通用的暂停语义；需要时由 Agent 取消）；handler 上下文不提供优先级（App 暂无
      按优先级降级的需求，需要时追加）；"用户在场"由 Agent 判断，本库不推断；Hub 侧唤醒不区分优先级（后台调用也会唤醒休眠 App——
      是否让后台调用不唤醒属于策略；P2 策略规则目前不能按优先级匹配，需要时给规则加该条件）。
    - 风险：Agent 把所有调用都标为 interactive 时退化为原来的到达顺序（无害）；让路只发生在队列满时（默认 64）。
- **P7 Hub 自身状态作为资源**：已连接 App、任务、句柄、配额余量以只读 MCP 资源暴露（K13），Agent 用 `read` 自查。
  - **实施（2026-10-03）**：契约见 `spec/hub-api.md` 3.6「Hub 状态资源」；`crates/hub/src/hub_state.rs`。
    - 事实：两个资源 `app-mcp://apps/hub`（App 概况 + 所有未到期锁，持有者只给记账主体）与 `app-mcp://apps/self`（读取方自己的任务
      及其句柄、锁、用量、配额余量）。`apps` 是保留 appId（`RESERVED_APP_IDS`），不会与 App 资源冲突；读取复用 `status()`、
      `task_statuses()`、`lock_status()` 与记账表，配额余量由新增的 `RateBook::agent_available` 只读计算（不扣令牌）。
      "自己"按调用方键判定（自身 + `<键>/<任务 ID>` 句柄），不按记账主体——同一 `local` 主体下的其他 legacy 会话互不可见。
    - 取舍：不含任务 ID 与调用方键（句柄是凭据，`object_lock.rs` 的 @security 约定）；不可订阅（状态随每次调用变化，推送会给 Hub
      增加流量，与原则 4 不符，Agent 需要时再读）；不进 Hub API `resources()`（嵌入方有 `status()`），各语言绑定无需改动。
    - 测试：`tests/agents.rs hub_state_resources_show_own_view`（列表、self 只含自己且不含任务 ID、其他 Agent / 本机主体的视图、
      配额余量、hub 视图的锁不含调用方键、未知名字、Hub API 读取、不可订阅；把归属判定改为恒真时失败）；`hub_state` 单元
      （归属前缀边界）；`limits` 单元（余量）；`host_e2e resources_read_subscribe_update` 列表顺序。
    - 未知 / 未做：句柄自身的视图（资源读取不带句柄，只能经主体读取全部句柄）；Claude Code 是否会主动读这两个资源（取决于模型，
      资源描述已写明用途）；"句柄"（第 17 项）、订阅数尚未进入 self 视图（实施第 17 项时加）。
    - 风险：每次读取 `hub` 都构建一次完整 `status()`（含工具声明），只在 Agent 读取时发生，无常驻开销。

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
    - 测试：`crates/hub/tests/it/agents.rs` 5 个（两个 Agent 令牌的任务与句柄归属、句柄上限按 Agent、legacy 会话记身份、`/agents` 替换与
      Agent 令牌不能读 `/status`、启动校验；把主体恒置为本机时其中 3 个失败）；`agents.rs` / `task.rs` / `http_server.rs` 单元；Host
      `serve.rs agent_registry_cli_and_tokens`、`agents.rs` 与 doctor 单元。
    - 未知 / 未做：stdio 与 `serve_mcp_stream`（含移动端 Binder 上的 MCP）没有 HTTP 头，恒为本机主体，嵌入式 Hub 的调用方识别随第 4g e 项；
      `setup` 写入 Agent 配置时尚不为每个 Agent 自动登记令牌；IPC 上出示 Agent 令牌
      只是身份声明（同一用户本来能读 `agents.json`），不构成隔离。
    - 风险：令牌泄露即可冒用该 Agent 的身份——只影响区分与归属，不扩大权限（Agent 令牌不能访问 `/status`、`/policy`、`/agents`）；
      文件以 0600 写入，doctor 检出权限过宽。
    - 绑定（2026-10-03）：嵌入式 Hub 各语言绑定可登记 Agent（spec/hub-api.md 3.6「Agent 身份」的「绑定」）——hub-c 配置 `agents` +
      `am_hub_set_agents`（头文件 v19）、`@app-mcp/hub` `agents` / `setAgents`、uniffi `HubConfig.agents` / `set_agents`（Kotlin / Swift
      `setAgents`、Python `agents=` / `set_agents`）、C# `HubOptions.Agents` / `SetAgents`。各绑定一个端到端测试：经 `/mcp` 出示令牌的
      `apps.task.begin` 任务带对应 Agent、替换不合法时保留之前的登记、启动时不合法报配置错误且信息不含令牌。uniffi 生成的 Kotlin / Swift /
      Python 记录类型的字符串形式含令牌（生成代码不可定制），文档提示不要记录 `AgentCredential`；Swift 未编译验证（无 macOS）。
- **N6 并发仲裁**：SDK 提供 `busy()` / 对象锁；Hub 对写调用排队或返回明确错误；多会话对同一 App 公平排队。
  工具可声明 `concurrency: N` / `exclusive`（同一资源互斥），SDK 按声明排队，队列上限可配置，满时返回明确错误（4f k，由 4f 实施）。
  - 已实施（2026-10-03，Hub 对象锁）：`apps.lock {appId, key?, ttlMs?}` / `apps.unlock`（spec/hub-api.md 3.6「对象锁」）。锁存放在持有者的
    Agent 任务上（`crates/hub/src/task/locks.rs`），任务结束（会话关闭、`reset_session`、`apps.task.end`、空闲回收）即释放；`ttlMs`
    1–600 s（默认 60 s），到期在取用时判定、不设定时器。App 锁拦截其他持有者对该 App 的写调用（生效注解不是 `readOnlyHint: true` 的工具、
    上游工具、`apps.navigate`），在策略之后、限流之前返回新错误类别 `LOCKED`（-31003，`data {appId, key?, holder, retryAfterMs}`，
    `holder` 只给记账主体，不给任务 ID）；命名锁（`key`）只与同名加锁冲突、不拦截调用。Hub 不排队（"返回明确错误"一支）：等待与重试是
    Agent 的策略。`HubConfig.max_locks`（每持有者，默认 16，0 关闭并不列出）；`/status` `locks`；Host `mcp.maxLocks` / `--max-locks`，
    status 摘要与 doctor 列出持有中的锁。
    - 事实：内置工具经 `call_builtin` 分派、参数按 inputSchema 校验；写调用的准入顺序统一为 `admit_call`（策略 → 锁 → 资源保护），
      `apps.navigate` 单独检查；工具只读与否取 `tool_annotations`（App 声明 / 快照 / 页面目录 / 上游缓存），取不到按写处理。
    - 未知 / 未做：工具声明 `concurrency` / `exclusive` 与 SDK 侧按声明排队（4f k）已于同日实施（见下条）；
      `busy()`；人与 Agent 之间的仲裁（用户在 App 内的直接操作不经 Hub，锁拦不住，由 App 自己决定）；同一主体下不带句柄的多个客户端
      视为同一持有者（需互斥时各自 `apps.task.begin`）；锁不持久化（Hub 重启即全部释放）。
    - 风险：异常 Agent 反复加锁占住 App——每把锁至多 10 分钟、每持有者至多 `max_locks` 把，任务空闲回收即释放；内置工具多了两个
      （列表变长，`max_locks = 0` 可关闭）。
    - 补充（2026-10-03）：持有者与被拒绝方同属一个主体（如主体任务与其任务句柄）时 `LOCKED` 消息单独说明"同一主体的另一个任务"
      并指引用持有锁的句柄调用（`data` 不变）。
  - 已实施（2026-10-03，SDK 侧调度，4f k）：工具声明 `concurrency`（本工具同时执行的调用上限，0 = 不单独限制）与 `exclusive`
    （互斥组）、客户端配置 `maxQueuedCalls`（默认 64，0 = 不限），契约见 spec/protocol.md 5.3。
    - 事实：调度在 sans-IO 核心的调用队列（`crates/core/src/calls.rs` `can_start`、`connection/requests.rs` `pump_calls`）：按到达顺序
      扫描，因本工具 / 互斥组正忙而不能开始的调用留在原位，其后能开始的先开始（不被队头阻塞）；新到的调用需要排队且队列超限 →
      `RATE_LIMITED`（`data {scope: "queue", limit}`，不带 `retryAfterMs`），未开始、不进去重表。声明只在 SDK 内，不进
      `tools/sync` 与 `toolsHash`；只改声明不发 `tools/changed`，放宽后立即重新调度。复用已有错误类别，未新增 `CoreError` 变体
      （`concurrency` 用 0 表示不限、互斥组名按工具名规则校验）。
    - 接入：native `ToolOptions.concurrency / exclusive`、`NativeConfig.max_queued_calls`；C ABI v18（`AmToolOptions` / `AmClientOptions`
      末尾追加，按 struct_size 读取）；uniffi `ToolSpec` / `ClientConfig` 末字段；napi `ToolSpecInit` / 配置；WASM JSON 字段；tauri 插件
      页面消息。一致性：fake_host 新增 `--no-wait`（用例 `noWait`），用例 `call-scheduling` / `call-queue-limit`（能力 `callScheduling`）。
    - 测试：核心单元（`can_start`）与 `tests/client/calls.rs` 4 个（按工具上限不阻塞其他工具、互斥组跨工具串行且按序、队列超限、
      不限与放宽后开始）；wasm / C 转换单元；Rust runner 两个用例通过。变异：去掉互斥判断、去掉并发判断、去掉超限拒绝分别被检出。
    - 未知 / 未做：`busy()` 已于同日实施（见下条）；Hub 不知道 App 的调度声明（Agent 只在被拒绝时得知）。
    - 风险：扫描队列为 O(排队数 × 执行中数)，排队上限默认 64，开销可忽略。
  - 已实施（2026-10-03，用户正在操作 `setBusy`，由机主决定"排队和拒绝可用户配置"）：App 声明用户正在操作期间，写调用按客户端配置
    `busyPolicy` 拒绝（默认，`RATE_LIMITED` `data {scope: "busy"}`）或排队；只读调用、已开始的调用、导航与资源读取不受影响；
    策略可在运行时修改（`setBusyPolicy`，如 App 设置页让用户选择）。契约见 spec/protocol.md 5.3「用户正在操作」。
    - 事实：机制在 sans-IO 核心（`crates/core/src/connection/requests.rs` `busy_blocks` / `settle_busy`）：拒绝策略下新到与排队中的写调用
      随即被拒绝（未开始、不进去重表）；排队策略下写调用留在队列（`pump_calls` 跳过、不阻塞其后的调用），`set_busy(false)` 后按序开始。
      "写"按生效注解（`ToolAnnotations::effective`，与 Hub 对象锁相同口径）。busy 只在 SDK 内，不发给 Host。复用 `RATE_LIMITED`，
      未新增错误类别。何时算"正在操作"由 App 决定（P-08：不自动推断 UI 焦点 / 编辑状态，U7）。
    - 接入：native `set_busy` / `is_busy` / `set_busy_policy`、`NativeConfig.busy_policy`；C ABI v19（`am_client_set_busy` /
      `am_client_is_busy` / `am_client_set_busy_policy`、`AmBusyPolicy`；结构体不变）；uniffi `BusyPolicy` 与 `ClientConfig.busy_policy`
      末字段；napi / WASM `busyPolicy: 'reject' | 'queue'`；Tauri 插件页面 op `busy.set`（按页记录、取或，页面刷新 / 关闭即失效，
      只在汇总值变化时设置客户端，不覆盖 Rust 侧直接设置）。一致性用例 `call-busy-reject` / `call-busy-queue`（能力 `busy`）。
    - 测试：核心 `busy_rejects_write_calls_by_default`、`busy_queue_policy_defers_write_calls`；C ABI 往返与非法策略；wasm 配置解析；
      Tauri 按页汇总。变异：去掉写调用判断、去掉拒绝分支分别被检出。
    - busy 期间的 `app/navigate`（2026-10-03 实施）：导航会切走用户正在看的界面，同样按 `busyPolicy`——拒绝 → `RATE_LIMITED`
      `scope: "busy"`；排队 → 推迟到 `setBusy(false)`，到 Host 给的 `timeoutMs`（`NavigateParams` 新增可选字段，Hub 填导航等待剩余时间）
      仍在操作则回复 `NAVIGATION_FAILED`（`timeout`）、不再导航；没带 `timeoutMs`（旧 Host）按拒绝处理，避免 Host 放弃后才切换界面。
      Hub 把 App 回复的 `RATE_LIMITED` 原样交给 Agent（原先归为 `NAVIGATION_FAILED`）。测试：核心 `busy_rejects_navigate_by_default`、
      `busy_queue_defers_navigate_until_idle`、`deferred_navigate_expires`；Hub `navigation.rs busy_app_defers_or_rejects_navigation`。
      变异：去掉 busy 判断、去掉到期处理、去掉改策略时的拒绝、Hub 不透传 `RATE_LIMITED`、不带 `timeoutMs` 分别被检出。
      风险：Agent 取消调用后 Hub 不通知 App，推迟的导航仍可能在 `timeoutMs` 内执行（协议没有导航取消消息）。
    - 未知 / 未做：Hub 不知道 App 的 busy 状态
      （Agent 只在被拒绝时得知）；排队策略下用户操作很久时调用以 `TIMEOUT` 结束。
    - 风险：App 忘记撤销 busy 导致写调用一直被拒——错误消息与 `data.scope` 指明原因；Tauri / Electron 页面卸载自动撤销。

### 第四部分：场景扩展

- **N3 事件与触发器**：App 在清单声明可发出的事件；用户 / Agent 注册"事件 → 提示"规则，Hub 在事件到达时回调 Agent 宿主（Hub SDK 回调；Host 侧经 MCP 通知）；本库只投递事件，不代 Agent 发起调用（调用及其授权由 Agent 负责）。
  - **设计（2026-10-03，含 P4；机主已确认：拉取为主 + 通知提醒、休眠时丢弃并告知 App、一期 Rust + 二期各语言）**
    - 声明：清单 `events: [{name, description, payloadSchema?}]`（同工具名规则）；运行时声明经 `tools/sync` 同一时机的新通知
      `events/sync {events}`（无清单的网页 / 原生 App 也能声明）。未声明的事件 Hub 丢弃并记诊断。
    - 发出：SDK `emitEvent(name, payload?)` → 通知 `events/emit {name, payload?, eventId}`（`eventId` SDK 生成，Hub 去重）。
      只在已连接时发；休眠 / 未连接时 SDK **不缓存、不为发事件回连**（原则 4：事件不让 App 多一个连接或定时器），丢弃并返回
      `false` 让 App 知道。载荷受 `max_event_bytes`（默认 8 KiB）约束，超限 Hub 丢弃。
    - 订阅（"规则"）：Agent 用内置工具 `apps.events.subscribe {appId, event?, filter?}` / `apps.events.unsubscribe`，
      订阅归属**主体**（已登记 Agent 名；匿名调用方归调用方键，随 P1 任务回收）。厂商 / 机主：Hub API `subscribe_events`
      与回调 `set_event_handler`（同 `set_approval_handler` 形态）；Host 侧规则也可写在 `<home>/events.json`（同 policy.json 读写）。
      不支持"事件 → 自动调用"（微内核：触发器只投递事件）。
    - 投递 = 信箱（P4 合并）：每个订阅主体一个信箱，事件到达即入箱；Agent 以内置工具 `apps.events {ack?}`（取件，**任何 MCP
      客户端都能用**）或资源 `app-mcp://apps/events`（订阅后收 `resources/updated` 作提醒）取件。不依赖 Claude Code 是否
      处理自定义通知（U10 的保守解：拉取为主、通知只作提醒）。`apps/self` 带未读数，Agent 每次接触都能看到。
    - 上限（B-07）：信箱条数 `max_inbox_events`（默认 100，满则丢最旧并计数）、TTL `inbox_ttl`（默认 24 小时，**读取时惰性清理**，
      不加定时器）、每订阅频率上限（默认 60 条 / 分钟，超出丢弃并计数，经第 14 项限流同一实现）；主体订阅数上限 32。
    - 持久化（P4）：已登记 Agent 的信箱与订阅写 `<state_dir>/inbox/<agent>.json`（照 `dormant_store.rs`：原子写、0600、版本号、
      上限、过期丢弃、损坏跳过；`state_dir` 为 None 时只在内存）。匿名调用方不持久化。
    - 可观测：`HubStatus.events`（各订阅的投递 / 丢弃计数）、doctor、`HubEvent::AppEvent`。
    - 分期：一期 = 协议 + 核心 + native + Hub + Host + 一致性用例；二期 = 各语言 App SDK `emitEvent` 与 Hub 封装。
    - 未知：Claude Code 是否展示 `resources/updated`（不影响正确性，拉取可用）；事件 `filter` 先只支持载荷顶层字段相等匹配。
    - 风险：事件风暴（频率上限 + 条数上限 + 丢弃计数，测试覆盖）；持久化写放大（每次变更同步写整个信箱文件，≤100 条、小文件；事件风暴已被频率上限截住）。
  - **实施（2026-10-03，一期 Rust）**：契约 spec/protocol.md 3.5、spec/manifest.md 2.4、spec/hub-api.md 3.17。
    - 事实：协议 `crates/protocol/src/messages/events.rs`（`EventInfo` / `EventsSyncParams` / `EventEmitParams`，8 KiB 上限）；核心
      `crates/core/src/events.rs`（`declare_event` / `remove_event` / `emit_event`，握手顺序 tools → resources → events/sync → visibility
      → ready，恢复握手照发；未连接 `Ok(false)` 无副作用、不推迟空闲休眠；`eventId` = `e<n>` 进程内单调）；原生 `NativeClient` 同名三方法；
      清单 `Manifest.events` 与校验；Hub `crates/hub/src/events/`（catalog / inbox / store / delivery / builtin），去重键（连接, eventId）
      （固定 instanceId 的 App 重启后计数重来，按实例去重会误丢）；资源定为 `app-mcp://apps/events`（在保留的 `apps` 下，避免与 appId
      `events` 冲突）；Host 摘要 / doctor「事件订阅 N 个（信箱积压 / 丢弃）」。
    - 测试：核心 14、原生 3、一致性用例 `event-emit`（Rust runner pass，其他 runner 按能力跳过）；Hub 单元 31 + `tests/events.rs`
      （真实 native App、按订阅方提醒、已登记 Agent 任务回收与 Hub 重启后仍可取件、厂商回调）；Host `callers_text_lists_event_subscriptions`。
      变异：App 侧 13 个、Hub 侧 19 个 + 清单 1 个全部检出。
  - **二期（2026-10-03，各语言）**：App SDK `declareEvent` / `removeEvent` / `emitEvent`——Web（WASM 加载前也可声明）、Node、Electron
      与 Tauri 页面（桥接 op `event.declare` / `event.remove` / `event.emit`，声明归页面、页面结束撤销、多页同名按引用重新声明）、
      鸿蒙、Python、Kotlin、Swift、C ABI v20（`am_client_declare_event` / `remove_event` / `emit_event`）、C++、C#、Dart、Flutter（`McpEvent`）；
      `@app-mcp/build` 选项 `events`。Hub 封装：`@app-mcp/hub` `setEventHandler` / `HubEvent appEvent` / `HubStatus.events`；hub-uniffi
      `AppEventHandler` / `HubEvent::AppEvent`（Python / Kotlin / Swift）；hub-c v22 `am_hub_set_app_event_cb`（`am_hub_set_event_cb` 已是
      Hub 事件流）、C# `SetEventHandler`。一致性 `event-emit` 11 个 runner 全 pass。各族新增测试均做变异验证（JS 17、uniffi 11、C ABI 10）。
      未做：hub-c / hub-uniffi 未暴露 `HubConfig.event_limits`（用默认上限）；React 不加专用 hook（用 `useAppMcp().emitEvent`）；
      页面与主进程 / Rust 侧同名声明是同一份，页面全部撤销时一并撤销。
    - 未知 / 未做（一期遗留）：
      `CoreError` 新变体映射到 `NativeError::InvalidName` / `InvalidJson`（加 `NativeError` 变体需各绑定同步）；Host `<home>/events.json`
      机主规则未做（厂商用 Hub API）；频率窗口不持久化；任务句柄的信箱与主体分开（取件需同一 `taskId`）；Claude Code 是否展示
      `resources/updated` 未验证（不影响拉取）。
- **N4 标准意图**：定义通用动词 schema（先试点 `message.send`、`calendar.create`、`media.play`、`file.share`、`navigation.open`），App 声明实现；
  Hub 按用户默认 App 路由；codegen 输出到系统意图框架。
  - **U4 调研结论（2026-10-04）**：四个来源都不完整——Apple App Schemas 无通用分享（`.messages.sendMessage`、`.calendar.createEvent`、
    `.audio.playAudio`、`.maps.startNavigation`、`.browser.openURLInTab`，多为 iOS 27，参数为 App 自定义 Entity）；Android AppFunctions
    预置 schema 模块已从 androidx-main 移除（1.0.0-alpha12，调用需特权），标准 Intent 五个都能覆盖；鸿蒙 API 20 标准意图只有媒体与导航
    （参数以 entityId / GCJ02 坐标为主）；schema.org Actions 不是 JSON Schema；MCP 无标准工具词表。仓库 codegen（swift-app-intents /
    kotlin-appfunctions / windows-app-actions / harmony-insight-intents）目前一个工具一个自定义意图，不对接任何系统 schema。
  - **设计（2026-10-04 机主确认：Agent 选 + 机主默认表作提示、拆为 6 个动词、codegen 系统 schema 放二期）**：自定义词表 + 映射表，一处定义 `spec/intents.md`（动词名 `<域>.<动作>`、版本、JSON Schema、各平台映射）；
    工具在清单 / 运行时声明 `implements: "message.send@1"`（只是声明，Hub 不校验参数是否"真的"实现语义，只校验 inputSchema 与词表兼容：
    词表必填字段在工具 schema 中存在）。Agent 侧：内置工具 `apps.intents {intent?}` 列出各动词的实现者（不唤醒）；调用仍按工具全名，
    Hub 不代选 App（"用户默认 App"属策略）。机制上提供可选的机主默认表（`<home>/intents.json` / Hub API），`apps.intents` 把默认实现排在
    首位并标注 `default: true`，由 Agent 决定是否采用。codegen 只在映射表有对应项时额外输出系统 schema 版本，二期做。
  - 试点动词（参数取各平台交集）：`message.send {to[], text, subject?, attachments?}`、`calendar.create {title, start, end?, allDay?,
    location?, attendees?, notes?}`、`media.play {query | uri, kind?}`、`file.share {files[], mimeType?, text?, to?}`；原 `navigation.open`
    拆为 `link.open {url}` 与 `navigation.start {destination{name?|address?|lat,lng}, mode?}`。
  - 未知：Apple Entity 型参数如何从平铺 JSON 生成；鸿蒙标准意图是否需平台审核；Xcode 未编译验证。
  - **实施（2026-10-04，一期 Rust）**：契约 spec/intents.md、spec/protocol.md `ToolInfo.implements`、spec/manifest.md。
    - 事实：词表与兼容性检查一处定义 `crates/protocol/src/intents.rs`（6 个动词 @1、`IntentRef`、`validate_implements`、`compatibility`）；
      清单校验（格式 / 重复 / 上限为错误，未知动词与不兼容为警告）；核心 `ToolDef` / `ToolUpdate.implements`、`CoreError::InvalidImplements`
      （原生映射 `NativeError::InvalidName`）；Hub `crates/hub/src/intents/`（collect / builtin / defaults）：`apps.intents`（复用 apps.search
      的候选来源，不唤醒、hide 过滤、默认排首位、不兼容列入 incompatible、未知 known:false、命中 App 记入暴露集合）、`HubConfig.intent_defaults`
      / `Hub::set_intent_defaults` / `Hub::intents` / `HubStatus.intents`、`HubTool.implements`（apps.search 结果带上）；Host `<home>/intents.json`
      （启动不合法拒绝启动、`POST /intents` 与 `app-mcp-host intents show|validate|reload|set|unset`、doctor）。绑定：WASM / Node / uniffi 透传
      `implements`；C ABI 未加字段（二期）。
    - 测试：协议 / 清单 / 核心 / Hub 单元，`crates/hub/tests/it/intents.rs`、host `serve.rs`；变异 29 个全检出（被中断的前任留下一处未恢复的变异，
      接手时从备份恢复并核对；host `run_to_exit` 加 30 秒上限，避免"应拒绝启动却启动"时测试挂死）。
    - 未做（二期）：各语言 SDK 声明 `implements` 的封装（C ABI 字段、Web types、Kotlin / Swift / Python / C++ / C# / Dart / 鸿蒙、`@app-mcp/build`）、
      Hub 封装 `set_intent_defaults` / `intents()` / `HubStatus.intents`；codegen 输出系统 schema；规范未写的上限（动词名 64、默认表 256、
      `intent` 参数 80 字符）与 `POST /intents`、`HubStatus.intents` 待补进 spec/hub-api.md。
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
