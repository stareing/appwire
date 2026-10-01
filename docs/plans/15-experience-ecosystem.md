# 15 体验与生态：可见、可撤销、可测、可扩展（计划）

> 状态：计划（2026-10-02）。目标来源为 `app-mcp-plan.md` 第 15 节 M2–M4 中尚未实现的条目与第 19 节待决问题；
> 本文件只负责差距分析与任务拆分，实施结果写入 `TASKS.md` 第 15 项。分四组，组间独立，可穿插在 4c / 4d 之间。

## 0. 差距一览

| 组 | 条目 | 现状（证据） | 设计出处 |
|---|---|---|---|
| X 用户体验 | X1 操作可见（"AI 正在操作"提示、控件高亮、后台调用留痕） | 无实现（全仓搜索无） | M2 2.5、14.2 第 7 条 |
| | X2 撤销（handler 可选 `undo`、`history.undo` 工具） | 无实现 | 14.2 第 5 条、M4 4.4 |
| | X3 Host 托盘界面（已连 App、配对 / 审批弹窗、审计查看） | 无实现 | 第 19 节问题 4 |
| | X4 `batch` 工具（一次调用多个工具） | Hub 无；仅 native 运行时内部有批量投递 | M2 2.12 |
| Y 开发者体验 | Y1 一致性测试套件 | 无统一套件，各 SDK 各测各的 | M2 2.13、11.6 |
| | Y2 DevTools 面板、调用录制与回放 | 无；`@app-mcp/inspect` 是网页 UI 检查兜底，不是调试面板 | M4 4.3 / 4.4 |
| | Y3 Vue / Svelte 适配 | 只有 React；`@app-mcp/store` 已支持 Pinia 状态，`@app-mcp/dom` 框架无关 | M2 2.6、M4 4.5 |
| | Y4 Agent 侧类型化客户端 | codegen 已有 TypeScript / Kotlin / Swift / Python / C# / Dart 目标（`crates/codegen/src/targets`），缺"Agent 调用侧"用法与文档 | M4 4.8 |
| Z 覆盖面 | Z1 未改造 App 导入（URI 协议、D-Bus introspection、`.desktop` Actions、Jump List、`.sdef`） | 无实现 | 10.5、M3 3.7、M4 4.6 |
| | Z2 OS 层（Windows UIA、macOS AX、Linux AT-SPI）L2 / L3 工具 | 无实现 | 第 9 节、M3 3.1–3.3、M4 4.1 |
| | Z3 浏览器扩展（Native Messaging、标签页激活、HTTPS 网页免直连 localhost） | 无实现；可同时解决 Chrome 本地网络访问授权与 `window.close()` 关不掉外部标签页 | M3 3.10 |
| R 远程 | R1 远程 / 跨设备 Agent | 只支持本机；鉴权需重新设计 | 第 19 节问题 6 |

## 1. 已知的已知

- 审批、审计、限流在第 14 项落地；X2 / X3 复用其审计记录与审批通道。
- `@app-mcp/web` 已有 WebMCP 兼容与 `undo` 相关接口雏形（`packages/web/src/webmcp/install.ts`），X2 的 SDK 侧从这里对齐。
- Hub 已有工具路由、渐进暴露（`apps.*`）与格式导出；X4 在 `apps.*` 命名空间下新增，不影响 App 工具。
- 唤醒、生命周期、按名寻址分别在 4e / 4d；Z1 导入的 App 走同一唤醒路径。
- `spec/naming.md` 已规划浏览器扩展作为 4d 平台之一（Z3 与之合并实施）。

## 2. 已知的未知

| # | 未知 | 处理 |
|---|---|---|
| U1 | 托盘实现选型（Tauri / 原生 Win32 + AppIndicator + NSStatusItem / 其他） | **技术验证**：比较体积、依赖、Linux 托盘可用性（GNOME 默认无托盘），结论写回本文件后再实施 |
| U2 | 一致性测试的驱动方式（各语言跑同一份 JSON 用例 vs 统一假 Hub 驱动各 SDK 示例进程） | **验证**：先用假 Hub + 用例表驱动 Python / Node 两个 SDK 试点，再推广 |
| U3 | `undo` 语义：哪些工具可撤销、撤销窗口、与 App 自身撤销栈的关系 | **保守**：只由 handler 显式提供；Hub 只记录可撤销调用并转发，不推断 |
| U4 | OS 层 L3 的权限要求（macOS 辅助功能授权、Linux AT-SPI 总线开启状态） | 各平台实测；macOS 无环境，记为待验证 |
| U5 | 远程 Agent 的鉴权与传输（TLS、设备配对、令牌轮换） | **单独设计文档**后再实施；默认关闭，不改变本机默认安全姿态 |
| U6 | Chrome 扩展上架与 Native Messaging 主机清单在各平台的注册位置 | 以 Chrome 官方文档为准逐平台核实，由第 13 项 `setup` 写入 |

## 3. 未知的未知（限制手段）

- 每项都有开关，默认值不扩大攻击面（远程默认关闭；OS 层工具默认需确认，风险等级 `os-sensitive`）。
- 新增 Hub 工具（`batch`、`history.undo`）同样经第 14 项审批与限流；`batch` 内每个子调用分别审批、分别审计，任一失败按声明策略停止或继续。
- 一致性测试先于新增 SDK 功能落地，后续行为改动以其为准（A-04）。

## 4. 任务与顺序

1. **Y1 一致性测试套件**（第 14 项之后首先做；4e 第二部分刚改动全部绑定，需要统一兜底）。
2. **X1、X4、Y3**（与 4c 穿插；X1 的网页侧与 4c 的界面级暴露共用"当前界面"概念）。
3. **X3 托盘**（U1 验证后）→ **X2 撤销**（依赖第 14 项审计）。
4. **Y2 DevTools 与录制回放**、**Y4 文档补齐**。
5. **Z3 浏览器扩展**（随 4d 的扩展平台）→ **Z1 导入器** → **Z2 OS 层**（M3 起，Windows 优先）。
6. **R1 远程 Agent**：先出设计文档，评审后决定是否实施。

## 5. 验收

- X：调用发生时 App 内可见提示；后台调用在托盘有记录；`history.undo` 能撤销声明了 `undo` 的调用；`batch` 子调用逐个审批与审计。
- Y：全部 SDK 通过同一套一致性用例；Vue Demo 可被模型操作；DevTools 面板可回放一次录制的调用序列。
- Z：Windows 上不改造记事本完成"写入剪贴板内容并保存"（设计 M3 验收）；装扩展后 HTTPS 网页无需直连 localhost。
- R：设计文档评审结论记录在本文件。
