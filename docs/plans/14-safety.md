# 14 职责边界：如实传递与资源保护（计划）

> 状态：计划（2026-10-02，同日按机主决定重写）。原版本（Host 侧确认、事先授权、无人值守审批、五维风险与 A–D 确认等级）已撤销，
> 见第 1 节"职责划分"。实施结果写入 `TASKS.md` 第 14 项。

## 0. 结论

操作分级、是否确认、事先授权、无人值守策略**不是本库职责**：

- **Agent**（Claude Code、Cursor、厂商 Agent）有用户界面、知道用户是否在场、有自己的权限 / 放行配置，负责"要不要问用户"。
- **App** 懂业务、有自己的界面与验证（支付密码、3DS、生物识别），负责高风险操作的最终确认（例如付款在 App 内确认）。
- **本库**是 Agent 与 App 之间的通道，只做两方都做不到的事：**如实传递 App 的声明**、**保护 App 与设备资源**、**保证通道本身的正确性**。

## 1. 职责划分

| 事项 | 归属 | 本库做什么 |
|---|---|---|
| 风险分级、要不要确认、"本会话允许"、事先授权 | Agent | 把 App 声明的标准 MCP 工具注解原样传给 Agent |
| 无人值守下能做什么 | Agent（运行模式由它决定） | 无 |
| 付款等高风险操作的最终确认 | App（自己的界面与验证） | 无；SDK 不提供确认组件，不定义"必须确认"语义 |
| 提示词注入防护策略 | Agent | 把 App 对内容的标注原样传递 |
| 调用频率、唤醒次数、数据大小 | **本库** | 限流、唤醒上限、大小上限（保护 App 与设备，B-07） |
| 句柄的访问范围 | **本库**（句柄由本库签发） | 句柄绑定会话、有 TTL、不可猜（第 17 项） |
| 调用记录 | **本库**（只有它看得到所有 Agent 的调用） | 调用日志用于排查，归第 11 项可观测，不作为安全审计 |
| 嵌入式 Hub 的审批 | 厂商（厂商就是 Agent） | 保留现有 `ApprovalHandler` 回调，作为第 16 项 P2 策略挂点在调用执行点上的回调形态 |
| 用户 / 厂商写的规则（隐藏、拒绝） | 用户 / 厂商 | 只提供执行点并执行规则，不内置判断（第 16 项 P2，2026-10-02 决定） |

## 2. 已知的已知

| # | 事实 | 证据 |
|---|---|---|
| K1 | 协议已有 `risk`（`read / write / destructive / payment / os-sensitive`，缺省 `write`） | `spec/protocol.md:182`；`crates/protocol/src/messages.rs` `Risk` |
| K2 | Hub 已把 `risk` 映射为 MCP 注解（`read` → 只读；`destructive` / `payment` → `destructiveHint`） | `crates/hub/src/call.rs` 约 935 行 |
| K3 | 导出描述在 risk 非 read / write 时追加"（风险：…）" | `spec/hub-api.md:593` |
| K4 | Hub SDK `ApprovalPolicy` 默认不审批，`ApprovalHandler` 供厂商接管 | `crates/hub/src/types.rs` |
| K5 | 已有唤醒速率上限（每 App 6 次 / 60 s） | 4e O4 `HubConfig.wake_rate_limit` |
| K6 | 工具按全名调用，Agent 可按工具名放行 / 拒绝；尚无通用调用入口 `apps.call` | `crates/hub/src` 内置工具仅 `apps.list / overview / select / tools`；`docs/plans/12-mcp-2026-07-28.md:112` |

## 3. 已知的未知

| # | 未知 | 处理 |
|---|---|---|
| U1 | Claude Code 等 Agent 是否读取 MCP 工具注解、对 MCP 工具是否默认逐次确认 | **已验证（2026-10-02，Claude Code 2.1.281，stdio 探针服务器 + `--permission-mode default` / `acceptEdits`）**：`readOnlyHint: true`、无注解、`destructiveHint: true` 三个工具同样被要求授权，注解**不会**自动放行（官方 issue anthropics/claude-code#87452 "auto-allow readOnlyHint" 以 not planned 关闭）；`readOnlyHint` 只影响计划模式能否调用与并行执行。结论：用户须在 Agent 中配置放行规则，本库照常如实声明注解（文档注明）。未测：经本库 HTTP Host 的路径、交互式界面的授权提示是否展示注解 |
| U2 | 第 12 项若引入 `apps.call`，Agent 只看到一个工具名，按工具放行失效 | **约束**：尽量不引入；必须引入时 `apps.call` 本身声明为最高注解（非只读、`destructiveHint`、`openWorldHint`），由 Agent 对它整体确认；本库不在其内部另做分级 |

## 4. 未知的已知（可复用）

- MCP `ToolAnnotations`（`readOnlyHint`、`destructiveHint`、`idempotentHint`、`openWorldHint`、`title`）是 Agent 侧通用的声明载体，rmcp 3.5.0 已支持。
- 现有 `risk` → 注解映射（K2）可作为兼容层保留。
- 4e 的唤醒上限与 `/status` 计数结构可直接承载限流计数。

## 5. 未知的未知（限制手段）

- 本库不做策略判断，因此不存在"判断错误放行"的风险面；残余风险是 App 声明不实——由 Agent 与用户对 App 的信任决定，本库在 `doctor` 中展示每个工具的声明以便核对。
- 限流与大小上限都有配置项和明确错误码；超限不静默丢弃。

## 6. 方案与任务

- **S1 标准注解声明**：App 注册 / 清单可直接声明 MCP 注解（`readOnlyHint`、`destructiveHint`、`idempotentHint`、`openWorldHint`），
  Hub 原样转发；`risk` 保留为兼容写法并按 K2 映射，文档标注为旧写法（E-06，不删除）。
- **S2 内容标注透传**：App 对工具结果 / 资源内容的标注（如"来自外部"）按 MCP 内容注解原样传递，本库不据此做决策。
- **S3 按工具限流**：令牌桶，按（App, 工具）与按 App 两级，默认值宽松、可配置；超出返回新错误码 `RATE_LIMITED`（`retryAfterMs`）。
- **S4 大小上限**：参数、结果、资源内容的单项上限（与第 17 项内容通道共用配置）；超出返回明确错误。
- **S5 `doctor` 展示声明**：列出每个工具的注解 / `risk`，便于用户与 Agent 配置放行规则。

不做（已撤销）：Host 侧确认（elicitation / 托盘弹窗）、事先授权（grant）与异步审批、无人值守判定、多维风险与确认等级推导、关键词升级启发式、
读取外部内容后的冷却策略、安全审计日志（调用日志并入第 11 项）。

## 7. 验收

- App 声明的注解在 Claude Code 的工具列表中可见且与声明一致；旧 `risk` 声明得到与现在相同的注解。
- 限流与大小上限触发时返回明确错误码；`doctor` 展示每个工具的声明。
- `spec/protocol.md`、`spec/manifest.md`、`spec/hub-api.md` 更新；cargo / pnpm 全量测试与 clippy 0。
