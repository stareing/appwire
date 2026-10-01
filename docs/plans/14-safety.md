# 14 安全基线：确认、审计、限流、防注入（计划）

> 状态：计划（2026-10-02）。威胁模型与目标措施见 `app-mcp-plan.md` 第 14 节；行为契约落在 `spec/hub-api.md`、`spec/protocol.md`，
> 本文件只负责差距分析与任务拆分，实施结果写入 `TASKS.md` 第 14 项。
> **优先级**：先于 4c；第 13 项（开箱即用）面向外部用户发布前必须完成第一部分。

## 0. 结论

经 Host 使用时，`payment` / `destructive` 工具目前**不经任何确认直接执行**（仅靠 Agent 是否尊重 `destructiveHint` 注解），
与设计第 14.2 节"`payment` 每次必确认"不符；同时没有审计记录、没有按工具限流、没有注入防护。四项均属开放给真实用户前的底线。

## 1. 已知的已知（代码可证的事实）

| # | 事实 | 证据 |
|---|---|---|
| K1 | `ApprovalPolicy` 默认 `require_at_or_above: None`（不审批） | `crates/hub/src/types.rs` `ApprovalPolicy` |
| K2 | 需要审批但无 `ApprovalHandler` → `USER_REJECTED`（不放行）；审批有超时，超时视为拒绝 | `crates/hub/src/call.rs` `approve` |
| K3 | Host 未设置审批策略与处理器 | `crates/host/src/*.rs` 无 approval 引用 |
| K4 | 风险等级 `read / write / destructive / payment / os-sensitive` 在协议中定义；Hub 映射为 MCP 注解（destructive / payment → `destructiveHint`） | `crates/protocol/src/messages.rs`；`crates/hub/src/call.rs` 约 935 行 |
| K5 | rmcp 3.5.0 服务端 `Peer::elicit` / `elicit_with_timeout` 可用 | `~/.cargo/registry/.../rmcp-3.5.0/src/service/server.rs:1144` |
| K6 | 现有限流只有唤醒速率上限（每 App 6 次 / 60 s） | 4e O4，`HubConfig.wake_rate_limit` |
| K7 | 网页 App 首次连接有配对确认，之后用令牌；`/mcp` 令牌策略默认 `browser` | `crates/core/src/lib.rs` `Paired`；`crates/host/src/config.rs` `AuthMode` |
| K8 | 已有 `DiagnosticReport` 与"最近错误"记录结构，可作为审计存储的参照 | `crates/hub/src/types.rs` |

## 2. 已知的未知

| # | 未知 | 处理 |
|---|---|---|
| U1 | Claude Code 等 Agent 是否声明 elicitation 能力、表单如何呈现 | **验证**：临时 Host 记录 `initialize` 的 `capabilities`（同第 12 项 U1 的方法）；不支持的 Agent 走 U2 的兜底 |
| U2 | Agent 不支持 elicitation 时的确认通道 | **保守**：`payment` 直接拒绝并在错误中说明原因；托盘确认框（第 15 项 X3）就绪后改为弹窗 |
| U3 | MCP 2026-07-28 无会话协议下 elicitation 的形态（服务端发起请求是否仍可用） | 与第 12 项一并设计；本项按 2025-11-25 实现，抽象为 `ApprovalHandler`，传输细节不外泄 |
| U4 | 审计日志的保留期与体积 | **配置化**：默认 30 天 / 50 MB 滚动，超出删除最旧；参数只存摘要 |
| U6 | `payment` 能否被事先授权（与设计 14.2"不可设为总是允许"的关系） | **待机主确认**；在确认前默认：`payment` 不可创建授权，无人值守下只能走异步审批；可选方案为"必须带单笔与周期金额上限、有效期 ≤ 30 天、仅限指定收款方"的受限授权，由配置项显式开启 |
| U7 | "用户在场"的可靠信号（锁屏、空闲时长、Agent 自报）各平台取法 | **保守**：以显式声明为准；锁屏 / 空闲超过阈值时降级为 `unattended`，不反向升级；各平台 API 实测后写回 |
| U8 | 手机推送审批通道的形态（伴侣 App vs 厂商回调） | 先只做桌面通知 / 托盘与 Hub SDK 回调；手机推送随第 13 项第三部分 / 4d 再定 |
| U5 | 防注入策略的误伤率（读外部内容后禁止高风险调用会打断正常流程） | **默认只对 `payment` / `os-sensitive` 生效**，其余等级可配置；记录触发次数供调整 |

## 3. 未知的已知（可复用）

- `ApprovalHandler` trait 与超时 / 取消路径已完整（K2），Host 只需提供实现，不改调用链。
- 各绑定（hub-c / hub-uniffi / hub-node / C#）已透传审批回调，厂商侧无需新接口。
- `/status`、`doctor` 已有 JSON 输出结构，审计与限流计数可挂在同一处。
- 协议风险等级已在清单与注册中声明，无需 App 改动。

## 4. 未知的未知（限制手段）

- **默认拒绝**：任何无法确认的高风险调用一律拒绝，错误信息说明原因与解除方法（不静默放行）。
- **开关**：每项都有配置项，可按 App / 按工具调整；回退需显式配置并在 `doctor` 中标为警告。
- **回归测试**：每种风险等级 × 有 / 无 elicitation × 同意 / 拒绝 / 超时 / 取消；限流边界；审计写入失败不阻塞调用但计数并告警。
- **敏感数据**：审计不记录完整参数，只记录键名、长度与哈希；令牌、`password` 等字段名一律打码（G-05）。

## 5. 方案与任务

### 第一部分：确认（P0）

- **S1 Host 审批默认值**：`require_at_or_above = destructive`；`payment` 每次必确认，不可设为总是允许；`os-sensitive` 支持"本次会话允许"。
- **S2 elicitation 审批处理器**：Host 以发起调用的 MCP 会话为通道，经 `elicit_with_timeout` 请求确认（工具名、App、参数摘要、风险）；
  该会话未声明 elicitation 时按 U2 处理。
- **S3 Hub SDK 默认值**：厂商嵌入时未设置 `ApprovalHandler` 且出现 `payment` 工具 → 启动日志警告；`payment` 调用拒绝（K2 已保证）。

### 第二部分：审计与限流

- **S4 审计日志**：`<home>/logs/audit.jsonl`，字段：时间、MCP 会话、Agent 名（`clientInfo`）、App / 实例、工具、风险、参数摘要、结果类别、耗时、审批结论。
  `app-mcp-host history`（`--json`、`--app`、`--since`）查看；Hub SDK 以回调暴露审计事件，存储由厂商决定。
- **S5 按工具限流**：令牌桶，按（App, 工具）与按 App 两级；默认 `read` 宽松、`write` 及以上收紧；超出返回新错误码 `RATE_LIMITED`（`retryAfterMs`）。

### 第三部分：防注入

- **S6 数据标记**：资源内容与工具返回值中来自用户 / 外部的内容加数据边界标记（MCP 内容注解），工具描述中不拼接外部文本。
- **S7 高风险冷却**：读取被标记为外部来源的内容后 N 次调用内（默认 3），`payment` / `os-sensitive` 必须确认且确认框注明"刚读取外部内容"。

### 第四部分：授权模式——无人值守与用户授权（2026-10-02 加入）

同一套判定同时支持"有人在场、实时确认"和"无人值守、事先授权"。授权模型只在本节定义；第 16 项 N5（Agent 身份）、
第 17 项 F1（选择即授权）、第 16 项 N3（触发器）引用本节，不另行定义。

**两种调用情境**（显式声明，不靠猜）：

| 情境 | 来源 | 能否实时确认 |
|---|---|---|
| `interactive` | 用户在 Agent 中对话发起 | 能（elicitation / 托盘弹窗） |
| `unattended` | 定时任务、事件触发器（第 16 项 N3）、无界面 Agent、CI；或 Agent 被用户配置为无人值守；屏幕锁定 / 用户长时间离开时保守视为此情境 | 不能，只能用事先授权或异步审批 |

**判定顺序**（单一入口，替换现 `approve` 的布尔判定；每一步结论写入审计，注明情境与依据）：

1. 拒绝规则命中 → 拒绝。
2. 有效授权（grant）命中且未超限 → 放行（审计记 `grant:<id>`），扣减用量。
3. `interactive` → 实时确认；用户可选"仅本次 / 本会话 / 创建授权"（`payment` 无"创建授权"，见 U6）。
4. `unattended` 且配置了异步审批通道 → 发出审批请求（桌面通知 / 托盘、手机推送），调用返回 `APPROVAL_PENDING`（带 `ticket`、`expiresAt`），
   Agent 之后凭 `ticket` 重试；同一 `callId` 幂等（第 16 项 N7a），不会执行两次。
5. 其余 → 拒绝，错误码 `UNATTENDED_NOT_AUTHORIZED`，信息说明需要哪条授权、如何创建。

**任务**：

- **S8 授权（grant）模型**：主体（Agent 身份，第 16 项 N5）× 范围（App / 工具 / 资源 / 文件夹或文件类型）× 风险上限 ×
  约束（次数、速率、时间窗、到期时间、参数约束如收件人白名单、金额上限）× 情境（仅 interactive / 也允许 unattended）。
  存于 `<home>/grants.json`（0600），每条有 ID、创建者、创建时间、用量计数；到期自动失效。
- **S9 授权管理**：`app-mcp-host grant add | list | revoke | pause | resume`；托盘界面（第 15 项 X3）同等操作；Hub SDK 提供授权存储接口，由厂商实现持久化。
  `pause` 是总开关：立即停止全部无人值守执行（进行中的调用取消），恢复需用户操作。
- **S10 审批决定扩展**：`ApprovalHandler` 的返回值由布尔扩展为决定枚举（`AllowOnce` / `AllowSession` / `CreateGrant(..)` / `Deny` / `Pending(ticket)`），
  旧的布尔实现按 `AllowOnce` / `Deny` 兼容（E-06）；`ApprovalRequest` 增加情境字段与 Agent 身份。
- **S11 异步审批**：审批票据存储、过期、凭票重试；通道可插拔（桌面通知、托盘、手机推送经伴侣 App 或厂商回调）。
- **S12 无人值守下的副作用约束**：
  - 唤醒只用 `headless` / `background`，不抢前台焦点；每次在托盘或通知中心留痕（设计 14.2 第 7 条）；
  - 文件访问不能弹 `files.pick`，只能用预授权文件夹 / 类型产生的句柄（第 17 项 H2）；
  - 像素级兜底（第 17 项 R2）在无人值守下一律禁用。
- **S13 异常熔断**：授权用量超过速率或出现连续失败 → 自动暂停该授权并通知用户；审计中标记。

### 验收

- 安全用例（设计第 15 节 M2 验收）：恶意 Origin 被拒、`payment` 必须确认、未配对无法调用；新增：无 elicitation 时 `payment` 被拒、审计可查、限流生效。
- 无人值守：无授权时调用被拒（`UNATTENDED_NOT_AUTHORIZED`，信息含所需授权）；授权命中放行且用量扣减、超限即拒；
  异步审批返回 `APPROVAL_PENDING`，批准后凭票重试只执行一次；`pause` 立即停止全部无人值守执行；无人值守唤醒不抢前台且留痕。
- `spec/hub-api.md`、`spec/protocol.md` 第 10 节更新；cargo / pnpm 全量测试与 clippy 0。
