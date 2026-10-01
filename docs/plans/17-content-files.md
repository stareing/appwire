# 17 通用内容与文件：多媒体结果、数据句柄、文件选择与逐级退让（计划）

> 状态：计划（2026-10-02）。目标：让模型经 AppWire 读取、操作、在 App 间传递任何内容（图片、音频、文件），
> 并覆盖未接 SDK 的 App。行为契约落在 `spec/protocol.md`（内容与句柄）、`spec/hub-api.md`（系统工具、Blob 存储）、
> `spec/manifest.md`（参数声明）；本文件只负责分析与任务拆分，实施结果写入 `TASKS.md` 第 17 项。
> 第 16 项 N1"数据句柄"由本项实现，第 16 项只引用，不另行定义。

## 0. 结论

App → Hub 一段只能传 JSON：工具结果被序列化为一段文本交给模型，资源读取不支持二进制。MCP 与 rmcp 一侧的多媒体内容块已就绪，
缺口在 AppWire 协议本身。补三层：**内容通道**（App 能返回多媒体）、**数据句柄**（大数据不经模型）、**选择即授权**（用户选中的才可访问），
再加**逐级退让路由**覆盖未接 SDK 的 App。

## 1. 已知的已知（代码可证的事实）

| # | 事实 | 证据 |
|---|---|---|
| K1 | 工具结果只有 `data`（JSON）与 `stateHints` | `spec/protocol.md` `ToolsInvokeResult`（245 行） |
| K2 | Hub 把 `data` 序列化为一个文本内容块，对象另放 `structuredContent` | `crates/hub/src/call.rs:969` `success_result` |
| K3 | 资源读取"二进制内容暂不支持" | `spec/protocol.md:426` |
| K4 | rmcp 3.5.0 内容块：`image`、`audio`、`resource`（内嵌）、`resource_link` | `~/.cargo/registry/.../rmcp-3.5.0/src/model/content.rs:284–307` |
| K5 | 确认与授权归 Agent / App，本库只如实传递声明并限流 | `crates/protocol/src/messages.rs` `Risk`；`docs/plans/14-safety.md` |
| K6 | 调用路径优先级已有设计（SDK → 系统意图 → …） | `app-mcp-plan.md` 10.6 |
| K7 | 导入器、OS 层（UIA / AX / AT-SPI）在第 15 项 Z1 / Z2；句柄访问范围在第 16 项 N2 | `docs/plans/15-experience-ecosystem.md`、`16-agent-os.md` |

## 2. 已知的未知

| # | 未知 | 处理 |
|---|---|---|
| U1 | Claude Code 等 Agent 是否渲染工具结果中的 `image` / `audio` / `resource_link` | **验证**：临时 Host 返回各类内容块，在 Claude Code 实测；不支持的类型降级为文本描述 + 句柄 |
| U2 | WebSocket / IPC 单帧大小上限与大块 base64 的内存开销 | **测量**后定阈值：小于阈值内联，超过走句柄分块传输；阈值配置化 |
| U3 | Android `grantUriPermission` 的跨进程有效期、接收方被杀后的行为 | 真机验证（魅族 18 Pro）；并入批量真机测试 |
| U4 | iOS 文件提供者 / 安全作用域书签在 App 间传递的可行形态 | 无环境，**记为待验证**；iOS 先只支持 App 内句柄 |
| U5 | Linux Flatpak / Snap 下 xdg-desktop-portal Documents 门户的可用性 | Fedora / Ubuntu 各实测一次 |
| U6 | 系统分享目标注册方式（Android `ACTION_SEND`、iOS 分享扩展、Windows 分享契约）对 Host / 伴侣 App 的要求 | 各平台官方文档核实后定；桌面无统一分享时只做 `share.send` |
| U7 | 像素级兜底（截图 + 视觉）的准确率与误操作率 | 默认关闭（配置开关）；开启后如实声明注解，确认由 Agent 决定；记录成功率 |

## 3. 未知的已知（可复用）

- MCP 内容块与 rmcp 构造函数（K4）→ Hub 出口只需映射，不自造格式。
- `structuredContent` 已用于对象结果（K2）→ 新 `content` 与旧 `data` 并存，旧 SDK 无感。
- 第 14 项注解透传 → 剪贴板、像素操作等系统工具如实声明注解，由 Agent 决定是否确认。
- 第 16 项信息流标签 → 句柄携带来源标签，规则统一适用。
- 生命周期唤醒 → 消费句柄的 App 未运行时按现有路径唤醒后交付。
- codegen 已有各语言目标 → 参数中的文件类型生成为各语言的原生文件类型（`Uri`、`URL`、`Blob` 等）。

## 4. 未知的未知（限制手段）

- **句柄即能力**：随机不可猜、不可枚举；区分只读 / 读写；有 TTL；绑定 MCP 会话与 Agent（第 16 项 N5）；过期即删。
- **大小与类型**：单项与总量上限；按内容嗅探 MIME（不信扩展名）；图片可选去 EXIF。
- **默认关闭**：像素级兜底、剪贴板读取默认关闭（配置开关）。
- **调用日志**：每次句柄创建、交付、过期写入第 11 项调用日志。
- **回归测试**：句柄越权 / 过期 / 跨会话访问被拒；超限拒绝；降级路径（Agent 不支持图片时）。

## 5. 方案与任务

### 第一部分：内容通道

- **C1 多媒体结果**：`ToolsInvokeResult.content?: ContentBlock[]`（`text` / `image` / `audio` / `resource_link` / `resource`），与 `data` 并存；
  Hub 映射为 MCP 内容块（K4）。各 SDK 提供构造辅助（`Content.image(bytes, mime)` 等）。
- **C2 二进制资源**：`ResourcesReadResult` 支持 `blob`（base64）+ `mimeType`；删除 K3 的限制说明。
- **C3 模型侧适配**：大图生成缩略图交给模型、原图转句柄（依赖第二部分）；可选去 EXIF；按 U1 结论对不支持的 Agent 降级。

### 第二部分：Blob 存储与句柄（即第 16 项 N1）

- **H1 Hub Blob 存储**：内容哈希寻址、`appmcp-blob://<id>`、TTL、配额、绑定会话与授权；大块分块传输（U2 阈值）。
- **H2 参数声明**：schema `format: "appmcp-file"` + `contentMediaType`（清单与注册均可声明）；Hub 校验句柄类型与权限后交付。
- **H3 平台交付**：

  | 平台 | 交付方式 |
  |---|---|
  | Android | `content://` URI + `grantUriPermission`（临时，用后撤销） |
  | iOS | 先限 App 内（U4） |
  | Windows / macOS / Linux 原生 | 临时文件（0600）或文件描述符传递 |
  | Linux Flatpak / Snap | xdg-desktop-portal Documents（U5） |
  | 网页 | `Blob` / `File` |

### 第三部分：系统工具（选择即授权）

- **F1 `files.pick` / `files.save`**：Host 弹出系统原生选择 / 保存对话框，用户选择即授权，结果为句柄；无人在场时选择框无人操作，按超时返回取消（本库不判断是否有人值守）。
- **F2 分享**：`share.send(句柄)` 调起系统分享面板；Hub 注册为分享目标，外部分享进来的内容成为句柄（U6）。
- **F3 剪贴板**：`clipboard.read` / `clipboard.write`（文本、图片），风险 `os-sensitive`。

### 第四部分：逐级退让路由

- **R1 能力等级**：每个 App 标注可用的最高级：① SDK 工具 ② 系统意图（第 15 项 Z1） ③ 无障碍控件树（第 15 项 Z2，含 Android AccessibilityService）
  ④ 截图 + 视觉（像素级，复用 C1 图片通道，动作经无障碍接口注入）。`apps.list` 显示等级，Hub 按最高可用级路由。
- **R2 像素级兜底**：默认关闭（配置开关），如实声明注解，记录成功率（U7）。

## 6. 顺序与验收

1. 第一部分 C1、C2 先做（协议增量、无平台依赖）；C3 随第二部分。
2. 第二部分 H1、H2 → H3 按平台推进（Android 随真机批次）。
3. 第三部分 F1 先于 F2、F3。
4. 第四部分依赖第 15 项 Z1 / Z2，最后做。

验收：
- 相册 App 返回的图片在 Claude Code 中可见（U1 支持时），原图以句柄传给修图 App 裁剪，再传给聊天 App 发送，全程原图不进入模型上下文。
- `files.pick` 选择的文件可交给任一声明接受文件的工具；未选择的文件不可访问。
- Android 上跨 App 交付经 `content://` 临时授权完成，用后撤销。
- 句柄越权、过期、超限的回归测试全部通过；调用日志可查每个句柄的流向。
