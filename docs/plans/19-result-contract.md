# 19 结果契约：调用成功的完整标识与返回信息（计划）

> 状态：计划（2026-10-02）。面向 App 开发者与 Agent 开发者：一次调用"成功"到底说明了什么、返回了哪些可依赖的信息。
> 行为契约落在 `spec/protocol.md`（`ToolInfo`、`ToolsInvokeResult`、第 4 节错误表）与 `spec/hub-api.md`（`CallOutcome`、MCP 出口映射）；
> 本文件只负责分析与任务拆分，实施结果写入 `TASKS.md` 第 19 项。多媒体内容块归第 17 项 C1，此处只引用。

## 0. 结论

成败二分已有且分类清楚（JSON-RPC result / error，15 个 `kind`），但"成功"只表示 **handler 正常返回**，不表示业务完成；
返回值没有契约（无输出 schema）；MCP 出口丢掉了调用元信息；能否重试没有统一标注。补六项（R1–R6）。

## 1. 已知的已知

| # | 事实 | 证据 |
|---|---|---|
| K1 | 调用结果 `ToolsInvokeResult { data: unknown; stateHints?: string[] }`；失败为 JSON-RPC 错误，`data.kind` 共 15 类，`message` 面向模型 | `spec/protocol.md` 第 3 节（约 244 行）、第 4 节 |
| K2 | `ToolInfo` 无输出 schema；Hub 仅在 `data` 为对象时填 `structuredContent` | `spec/protocol.md` `ToolInfo`；`crates/hub/src/call.rs:970-990` `success_result` |
| K3 | 网页 SDK 中 handler 返回 `undefined` → `data: null`，模型看到文本 `null` | `packages/web/src/driver.ts:130-134` `toJsonValue` |
| K4 | MCP 出口成功结果只有内容块 + `structuredContent`，不带 `callId`、实例、耗时、是否唤醒 | `crates/hub/src/call.rs:970-990` |
| K5 | Hub API `CallOutcome` 带 `call_id`、`result`、`state_hints`、`instance_id` | `crates/hub/src/types.rs:259` |
| K6 | `stateHints` 在 MCP 出口追加为一段中文文本，非结构化 | `crates/hub/src/call.rs:980` |
| K7 | `retryAfterMs` 只出现在个别错误（如唤醒限流 `WAKE_RATE_LIMITED`），无统一"可重试"定义 | `spec/protocol.md:625` |
| K8 | 付款等高风险操作的最终确认归 App（handler 返回时可能仍在等待用户在 App 内确认） | `docs/plans/14-safety.md` 第 1 节 |
| K9 | 第 14 项 S1 / S2 正在实施：`ToolInfo.annotations`（MCP 工具注解）、`ToolsInvokeResult.annotations`（内容注解）已出现在工作区未提交改动中 | `git diff crates/protocol/src/messages.rs`（2026-10-02） |

## 2. 已知的未知

| # | 未知 | 处理 |
|---|---|---|
| U1 | 各 Agent 是否读取 MCP `outputSchema`、`structuredContent`、`_meta` | **验证**：临时 Host 在 Claude Code 实测；结论只影响文档建议，Hub 按 MCP 规范输出 |
| U2 | 非网页 SDK（C ABI、uniffi、Node、Dart）对"无返回值"的处理是否与 K3 一致 | **已验证（8b54b50，Y1 一致性套件）**：各 SDK 无返回值都发 `{data: null}`，与 spec 一致；Hub 侧按 R3 输出"已完成" |
| U3 | MCP 2026-07-28 `_meta` 键命名约定（反向域名前缀）与 `resultType` 的关系 | **核实**规范（第 12 项 F1）后定键名，未确认前不落地 R4 |
| U4 | `pending` 结果后续状态的查询载体（状态资源 vs 第 16 项 P5 作业） | **保守**：先用 App 声明的状态资源名；P5 落地后增加作业 ID |

## 3. 未知的已知（可复用）

- 第 14 项 S1 / S2 正在改 `ToolInfo` / `ToolsInvokeResult`（K9）→ R1、R2、R3 与之**合并成同一批契约变更**，只升级一次协议。
- MCP `Tool.outputSchema`、`CallToolResult.structuredContent` / `_meta` 由 rmcp 3.5.0 支持 → Hub 只做映射。
- 错误码表单一定义在 `spec/protocol.md` 第 4 / 10 节 → R5 只加一列。
- 第 17 项 C1 内容块（`resource_link`）→ R6 直接复用。

## 4. 未知的未知（限制手段）

- 所有新字段可选、缺省等于现行为（E-06）；旧 SDK、旧 Agent 不受影响。
- R2 只做 Hub 侧可选校验（默认只记日志、不拒绝），避免 App 声明不准导致调用失败；开关可配置。
- 每项附一致性用例（Y1），覆盖每种 `status`、无返回值、非对象返回、元信息、可重试标注。

## 5. 方案与任务

- **R1 结果状态**：`ToolsInvokeResult.status?: "done" | "pending" | "partial" | "noop"`（缺省 `done`）。`pending` 表示已受理、等待 App 内确认或异步完成，
  附 `stateResource?`（App 资源名，U4）；`partial` 附说明。Hub 映射为结构化字段与一句文本，Agent 据此不把"已提交"当成"已完成"。
- **R2 输出 schema**：`ToolInfo.outputSchema?`（JSON Schema），Hub 转为 MCP `outputSchema`；非对象结果按 MCP 规范包装后放入 `structuredContent`；
  清单 `tools[].outputSchema` 同步（`spec/manifest.md`）；`@app-mcp/build` 与 codegen 从返回类型生成。
- **R3 结论摘要与无返回值**：`ToolsInvokeResult.summary?`（一句面向模型 / 用户的结论，受第 14 项 S4 大小上限）；
  无返回值时 Hub 输出固定文本"已完成"而非 `null`，`structuredContent` 不填。
- **R4 调用元信息**：MCP 出口 `_meta` 带 `callId`、`instanceId`、`durationMs`、`woke`（本次是否唤醒 App），键名按 U3；同一套键名规则还覆盖已落地的暂定键（7f587d8，前缀 `app-mcp/`，单一定义在 `crates/hub/src/names.rs` 与 `spec/hub-api.md` 3.15，改前缀只改这两处）：请求侧 `app-mcp/timeoutMs`（相对毫秒，Hub 取其与 `response_timeout` 的较小者，只限制等待 App 结果；第 16 项 P6 / 4f c）、`app-mcp/idempotencyKey`（1–256 字符，原样进 `ToolsInvokeParams.idempotencyKey`；第 16 项 N7a / 4f j），结果侧 `app-mcp/status`、`app-mcp/stateResource`（R1）、`app-mcp/routedTo`；R4 新增的 `callId` 等键沿用同一处定义；
  与 Host / SDK 日志 `cid` 可对照；Hub API `CallOutcome` 补 `duration_ms`、`woke`。
- **R5 可重试标注**：错误码表加"可重试"列（是 / 否 / 视 `retryAfterMs`），作为唯一定义；SDK 与 Hub 按表填 `data.retryable`。
- **R6 结构化状态提示**：`stateHints` 在 MCP 出口改为 `resource_link` 内容块（随第 17 项 C1），保留现有文本一个版本后移除（E-06）。

## 6. 顺序与验收

顺序：R1 + R2 + R3 与第 14 项 S1 / S2 同批（一次契约升级）→ R5 → R4（U3 核实后）→ R6（随第 17 项 C1）。

协调记录（2026-10-02，与实施第 14 项的会话确认）：R1–R3 已并入第 14 项 S1–S5 同一批协议升级，字段名按本文件原样（`status?` / `stateResource?`、`outputSchema?`、`summary?`），
R2 的 Hub 校验默认只记日志；R4–R6 不在该批。该批提交前，其他会话不改 `crates/protocol`、`spec/*` 与各 SDK。
该批已提交（a15c05d）；R1 在 MCP `_meta` 中暂用键 `app-mcp/status`、`app-mcp/stateResource`，R4 定键名时一并调整（U3）。
错误码编号规则（`spec/protocol.md` 第 4 节，单一定义）：-32000…-32019 已用的保持不变，-32020…-32099 属 MCP 规范，本库新增错误码从 -31001 起递增。
错误码分配：-32016 `RATE_LIMITED`、-32017 `PAYLOAD_TOO_LARGE`（第 14 项）；-32018 `POLICY_DENIED`（第 16 项 P2）、-32019 `USER_ACTION_REQUIRED`（第 18 项 L2）预留，不在该批。

验收：
- handler 返回 `{ status: "pending" }` 时，Claude Code 中的结果明确显示"已提交、待确认"；无返回值显示"已完成"而非 `null`。
- 声明 `outputSchema` 的工具在 MCP `tools/list` 中带 `outputSchema`，非对象结果也有 `structuredContent`。
- MCP 结果 `_meta.callId` 与 Host 日志 `cid` 一致；每种错误的 `retryable` 与错误码表一致。
- `spec/protocol.md`、`spec/manifest.md`、`spec/hub-api.md` 更新并在报告中写明契约变更；cargo / pnpm 全量测试与 clippy 0；Y1 一致性用例覆盖全部 SDK。
