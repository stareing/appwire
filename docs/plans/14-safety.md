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

### 验收

- 安全用例（设计第 15 节 M2 验收）：恶意 Origin 被拒、`payment` 必须确认、未配对无法调用；新增：无 elicitation 时 `payment` 被拒、审计可查、限流生效。
- `spec/hub-api.md`、`spec/protocol.md` 第 10 节更新；cargo / pnpm 全量测试与 clippy 0。
