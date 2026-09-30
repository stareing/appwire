# app-mcp 项目计划

> 一个零侵入的前端 / 桌面应用库：把 App 内的可交互能力以 MCP 工具和资源的形式暴露给大模型，让模型以结构化调用代替"截图 → 识别 → 点击"的方式操作 App 与操作系统。

- 状态：草案 v0.5
- 日期：2026-09-30
- 变更：
  - v0.5 新增第 20 节"Hub SDK（厂商接入）"、第 21 节"App 端生命周期"，附录顺延为第 22 节并加入 WebMCP、MCP-B、MCP-FE、tauri-plugin-mcp、Windows 原生 MCP 等对比与"做超集"的决定；WebMCP 描述按 2026-09-29 版规范修正，新增 6.7"WebMCP 兼容"；新增 8.6"注册为 Windows 智能体连接器"（按微软文档核实）；新增 11.8"接入注意事项"；同步更新包划分、里程碑、仓库结构
  - v0.4 新增第 12 节"网页与原生 App 的实现差异"；协议增加实例与可见性；安全增加对端进程校验；新增浏览器扩展桥接与 IPC 桥接；同步更新包划分、Host、里程碑、仓库结构、测试、风险与待决问题
  - v0.3 确定"Rust 核心 + 多语言绑定"架构，新增第 11 节"多语言与多 UI 框架策略"；Host 改为 Rust 实现；重写包划分、仓库结构与技术栈；同步更新协议、里程碑、测试、风险与待决问题
  - v0.2 新增第 10 节"跨平台唤醒与交互"，同步更新目标、包划分、协议、Host、安全、里程碑、风险与待决问题
- 名称：`app-mcp`（暂定）

---

## 目录

1. [背景与问题](#1-背景与问题)
2. [目标与非目标](#2-目标与非目标)
3. [设计原则](#3-设计原则)
4. [整体架构](#4-整体架构)
5. [包划分](#5-包划分)
6. [前端 SDK API 设计](#6-前端-sdk-api-设计)
7. [SDK 与 Host 通信协议](#7-sdk-与-host-通信协议)
8. [Host 设计](#8-host-设计)
9. [OS 能力分级](#9-os-能力分级)
10. [跨平台唤醒与交互](#10-跨平台唤醒与交互)
11. [多语言与多 UI 框架策略](#11-多语言与多-ui-框架策略)
12. [网页与原生 App 的实现差异](#12-网页与原生-app-的实现差异)
13. [零侵入与性能](#13-零侵入与性能)
14. [安全模型](#14-安全模型)
15. [里程碑与任务拆分](#15-里程碑与任务拆分)
16. [仓库结构与技术栈](#16-仓库结构与技术栈)
17. [测试策略](#17-测试策略)
18. [风险与应对](#18-风险与应对)
19. [待决问题](#19-待决问题)
20. [Hub SDK（厂商接入）](#20-hub-sdk厂商接入)
21. [App 端生命周期](#21-app-端生命周期)
22. [附录：相关方案对比](#22-附录相关方案对比)

---

## 1. 背景与问题

目前大模型操作 App 和操作系统主要有两种方式：

| 方式 | 问题 |
|---|---|
| **视觉操作**（computer use）：截图 → 识别 → 计算坐标 → 点击 → 再截图确认 | 每步数秒、单张截图消耗大量 token、容易点偏、受弹窗和加载状态干扰 |
| **无障碍树操作**（Playwright MCP、Chrome DevTools MCP 等） | 比视觉快，但粒度仍是"点击第 N 个按钮"，缺少业务语义，一个业务动作需要多轮调用 |

两者的共同问题：**模型面对的是界面元素，而不是 App 的业务能力**。

App 的开发者其实最清楚每个按钮背后的意图（"结算购物车""保存文档"），这些意图在代码里就是一个个 handler。只要把这些 handler 带上描述和参数 schema 登记出去，模型就能一步完成原本需要多轮视觉操作的任务。

```
现在：截图(~2k token) → 识别 → click(312,480) → 等待 → 截图 → 确认 …  每步数秒
目标：call shop.cart.checkout({ addressId: "3" }) → { orderId, total }   一次数十毫秒
```

## 2. 目标与非目标

### 2.1 目标

- **G1 零侵入**：接入后不改变 DOM、不引入额外渲染、不改变用户原有交互行为。
- **G2 同源**：人点击和模型调用执行同一个 handler，业务逻辑只维护一份。
- **G3 标准协议**：对外完全遵循 MCP，任意 MCP 客户端（Claude Desktop、Claude Code 等）无需定制即可使用。
- **G4 动态能力**：工具列表随路由、组件挂载、界面状态实时变化，并通知模型。
- **G5 可观察**：界面状态以 MCP 资源形式提供，支持订阅推送。
- **G6 OS 覆盖**：通过本地 Host 提供系统能力，并为未接入的第三方 App 提供无障碍树降级通道。
- **G7 安全可控**：风险分级、强制确认、本地配对鉴权、审计与撤销。
- **G8 按需唤醒**：App 未运行时工具依然可见，调用时按平台方式自动唤醒并回连，对模型透明。
- **G9 一次定义多端输出**：同一份工具定义可生成 MCP 工具、静态清单，以及各平台原生意图框架的声明。
- **G10 多语言一致**：核心逻辑只用 Rust 实现一份，通过绑定提供给 JS、C#、Swift、Kotlin、Dart、Python、C/C++，各语言行为一致。

### 2.2 非目标

- 不创造新的编程语言或 DSL。
- 不替代 App 的后端鉴权；库只在前端做能力登记，权限仍由原有后端决定。
- 第一阶段不做视觉识别（L4 兜底直接复用现有 computer use 方案）。
- 不做云端中转；所有通信只在本机进行。

## 3. 设计原则

1. **语义优先**：暴露"意图"（`cart.checkout`），而不是"元素"（`button#submit`）。
2. **默认不暴露**：只有开发者显式登记的能力才会暴露，避免工具爆炸和意外泄露。
3. **逐级降级**：L1 语义工具 → L2 系统原生能力 → L3 无障碍树 → L4 视觉，模型优先使用高级别能力。
4. **确认权在 Host**：危险操作的确认由 Host 用原生窗口完成，前端和模型都无法绕过。
5. **结果结构化**：工具返回有意义的结构化结果（新状态摘要），而不是只返回 `ok`。

## 4. 整体架构

```
┌──────────── 模型 / MCP 客户端（Claude Desktop、Claude Code …）────────────┐
                                   │ MCP（stdio / Streamable HTTP）
┌──────────────────────── app-mcp host（本地常驻进程）───────────────────────┐
│  ① 聚合路由：合并多个 App 的工具，加命名空间，如 shop.cart.checkout        │
│  ② 策略层：风险分级、原生确认弹窗、审计日志、限流                          │
│  ③ OS 能力（L2）：文件、剪贴板、窗口、进程、通知                           │
│  ④ 降级通道（L3）：Windows UIA / macOS AX / Linux AT-SPI                   │
└────────┬───────────────────────┬───────────────────────┬──────────────────┘
         │ WebSocket (127.0.0.1) │ IPC                   │ WebMCP（浏览器支持时）
   ┌─────┴──────┐          ┌─────┴───────┐          ┌─────┴──────┐
   │ Web App    │          │ Electron /  │          │ 浏览器内置 │
   │ + SDK      │          │ Tauri + SDK │          │ AI 直连    │
   └────────────┘          └─────────────┘          └────────────┘
```

**调用链路**（以 `shop.cart.checkout` 为例）：

1. 模型经 MCP 调用 Host 的 `tools/call`。
2. Host 解析命名空间，找到 `shop` 这个 App 连接。
3. 策略层判断风险等级为 `payment`，弹出原生确认框，用户确认。
4. Host 通过 WebSocket 把调用转发给前端 SDK。
5. SDK 在调用队列中执行原有 handler，界面照常更新。
6. 结果原路返回给模型，同时写入审计日志。

图中的 Host 由两部分组成：**Hub 库**（`app-mcp-hub`：注册表、路由、总览、App 连接服务、上游聚合、策略回调、MCP 出口与工具格式导出）
和很薄的**可执行外壳**（`app-mcp-host`：命令行解析 + `serve_stdio` / `serve_http`）。助手厂商可以不运行独立进程，
直接把 Hub 嵌进自己的产品（见第 20 节）。

## 5. 包划分

采用 **Rust 核心 + 多语言绑定**（设计细节见第 11 节）：协议、客户端核心、Host 全部用 Rust 实现；各语言只做绑定和一层符合语言习惯的薄封装。

### 5.1 Rust crates

| crate | 职责 | 阶段 |
|---|---|---|
| `app-mcp-protocol` | SDK ↔ Host 协议类型、序列化、版本协商；Host 与客户端共用 | M1 |
| `app-mcp-core` | 客户端核心（sans-IO）：注册表、scope、启用条件、调用队列与取消、连接状态机（握手、配对、重连、launchToken）、资源节流与差量 | M1 |
| `app-mcp-transport` | 传输实现：原生 WebSocket（tokio）、浏览器 WebSocket（wasm）、命名管道 / Unix socket、浏览器扩展 Native Messaging（Host 侧） | M1–M3 |
| `app-mcp-manifest` | 静态清单 `app-mcp.json` 的 schema、校验与读写 | M1 |
| `app-mcp-hub` | Hub 库（由 `app-mcp-host` 的实现迁出）：注册表、实例路由、总览、App 连接服务、上游 MCP 聚合、策略与审批 / 配对回调、MCP 出口（`rmcp`）、OpenAI / Anthropic / Gemini / MCP 工具格式导出与分派（第 20 节） | M2 |
| `app-mcp-host` | Host 可执行程序：M1 为完整实现；M2 起变为 Hub 之上的薄壳（命令行 + `Hub::serve_stdio` / `serve_http`） | M1 |
| `app-mcp-launcher` | 唤醒适配：URL、Windows URI / AUMID / exe、macOS bundle、Linux D-Bus / desktop | M2–M3 |
| `app-mcp-os-windows` | L2 系统能力 + L3 UI Automation | M3 |
| `app-mcp-os-macos` | L2 系统能力 + L3 Accessibility API | M4 |
| `app-mcp-os-linux` | L2 系统能力 + L3 AT-SPI | M4 |
| `app-mcp-importers` | 把已有能力导入为工具：URI 协议、macOS `.sdef`、D-Bus introspection、`.desktop` Actions | M3–M4 |
| `app-mcp-codegen` | 从清单生成各语言类型化接口，以及 App Intents、AppFunctions、Windows App Actions / ODR 连接器清单、D-Bus 声明 | M4–M5 |
| `app-mcp-conformance` | 协议一致性测试套件（可编排场景的假 Host） | M2 |

### 5.2 语言绑定与 SDK

| SDK（发布渠道） | 绑定技术 | 覆盖的 UI 技术 | 阶段 |
|---|---|---|---|
| `@app-mcp/web`（npm） | wasm-bindgen | 浏览器 Web App、Electron / Tauri 渲染进程 | M1 |
| `@app-mcp/node`（npm） | napi-rs | Electron 主进程、Node 工具 | M2 |
| Rust crate 直接依赖 | 无需绑定 | Tauri 后端、Slint、egui、Iced | M2 |
| `AppMcp`（NuGet） | uniffi + uniffi-bindgen-cs，或 C ABI + P/Invoke | WPF、WinUI、WinForms、MAUI、Avalonia、Unity | M3 |
| `AppMcp`（SwiftPM） | uniffi | SwiftUI、UIKit、AppKit | M4 |
| `app-mcp`（Maven） | uniffi | Jetpack Compose、Android View | M4 |
| `app_mcp`（pub.dev） | flutter_rust_bridge | Flutter（移动端与桌面） | M4 |
| `app-mcp`（PyPI） | uniffi 或 PyO3 | PyQt / PySide、Tkinter | M4 |
| C 头文件 + 动态库 | cbindgen（C ABI） | Qt、GTK、Win32、游戏引擎 | M4 |

5.1–5.2 都是 **App 端**（暴露工具的一方）。**Agent 端**（调用工具的一方，助手厂商嵌入）另有一组 Hub 绑定，接口契约见 `spec/hub-api.md`：

| Hub SDK（发布渠道） | 绑定技术 | 典型使用方 | 阶段 |
|---|---|---|---|
| `app-mcp-hub`（crates.io） | Rust 直接依赖（async API） | Rust 编写的助手、Tauri 助手 | M2 |
| `bindings/hub-c`（`app_mcp_hub.h`） | C ABI，前缀 `am_hub_` | C / C++ / C#（P/Invoke）/ Dart（dart:ffi）助手 | M2 |
| `bindings/hub-uniffi` | uniffi | Kotlin（Android 厂商语音助手）、Swift、Python Agent 框架 | M2 |
| `@app-mcp/hub`（npm，`bindings/hub-node`） | napi-rs | Electron 助手、Node Agent 框架 | M2 |

### 5.3 框架适配与工具

| 包 | 职责 | 阶段 |
|---|---|---|
| `@app-mcp/react` | `useTool`、`useResource`、`<ToolScope>`，基于 `@app-mcp/web` | M1 |
| `@app-mcp/extension` | 浏览器扩展（Chrome / Edge 优先）：网页 ↔ Host 桥接（Native Messaging）、标签页查找与激活、页面来源校验 | M3 |
| `@app-mcp/build` | 构建插件（Vite 优先）：从代码中的工具定义生成静态清单 | M1 |
| `@app-mcp/vue` | `useTool` / `useResource` composable、`v-tool` 指令 | M2 |
| `@app-mcp/electron` | 基于 `@app-mcp/node`，主进程接入，原生菜单 / 快捷键命令登记为工具 | M2 |
| `tauri-plugin-app-mcp` | Tauri 插件（Rust） | M3 |
| `@app-mcp/web/webmcp` | WebMCP 命令式 API 超集：polyfill / 桥接原生两种模式，工具双向镜像（6.7）；原计划的独立包 `@app-mcp/transport-webmcp` 并入此子入口 | M1（已提前完成） |
| `@app-mcp/devtools` | 浏览器扩展面板：查看已登记工具、调用日志、手动调用 | M4 |

## 6. 前端 SDK API 设计

### 6.1 React

```tsx
import { useTool, useResource, ToolScope } from '@app-mcp/react'
import { z } from 'zod'

function CartPage() {
  const { items, checkout, removeItem } = useCart()

  // 界面状态作为资源暴露，变化时自动推送
  useResource('cart.state', {
    description: '当前购物车内容与总价',
    read: () => ({
      items: items.map(i => ({ id: i.id, name: i.name, qty: i.qty, price: i.price })),
      total: sum(items),
    }),
    deps: [items],
  })

  // 复用原有 handler
  useTool('cart.removeItem', {
    description: '从购物车移除商品',
    input: z.object({ itemId: z.string().describe('商品 ID，来自 cart.state') }),
    risk: 'write',
    handler: ({ itemId }) => removeItem(itemId),
  })

  // enabled 为 false 时工具从列表中移除，并通知模型
  useTool('cart.checkout', {
    description: '结算当前购物车',
    input: z.object({ addressId: z.string() }),
    risk: 'payment',
    enabled: items.length > 0,
    handler: async ({ addressId }) => {
      const order = await checkout(addressId)
      return { orderId: order.id, total: order.total }
    },
  })

  return <ToolScope name="cart">{/* 原有 JSX 不变 */}</ToolScope>
}
```

### 6.2 初始化

```ts
import { createAppMcp } from '@app-mcp/web'

export const appMcp = createAppMcp({
  appId: 'shop',
  appName: '示例商城',
  enabled: import.meta.env.VITE_APP_MCP === 'on',  // 关闭时不加载 WASM 核心
  hostUrl: 'ws://127.0.0.1:7717',                  // WASM 核心在首次连接时按需加载
  highlight: true,  // 模型调用时高亮关联元素并显示提示条
})
```

注册 API 保持同步：WASM 核心加载完成前的注册先缓存在 JS 侧，加载后批量交给核心，不阻塞首屏。

`@app-mcp/web` 支持三种传输，自动按顺序选择（见 12.5）：

| 传输 | 适用场景 |
|---|---|
| 宿主 IPC 桥接 | 页面运行在 Electron、Tauri、WebView2、WKWebView 中 |
| 浏览器扩展桥接 | 普通浏览器，已安装 `@app-mcp/extension` |
| WebSocket 直连 | 普通浏览器，未安装扩展 |

### 6.3 原生 JS

```ts
const dispose = appMcp.tool('editor.save', {
  description: '保存当前文档',
  handler: () => editor.save(),
})
```

### 6.4 核心概念

| 概念 | 说明 |
|---|---|
| **Tool** | 一个可调用的业务动作：名称、描述、输入 schema、风险等级、handler、启用条件 |
| **Resource** | 一段可读、可订阅的界面状态，返回精简 JSON |
| **Scope** | 命名空间与生命周期边界；scope 卸载时其下所有工具和资源自动注销 |
| **Risk** | `read` / `write` / `destructive` / `payment` / `os-sensitive` |
| **Anchor**（可选） | 工具关联的 DOM 元素，用于调用时高亮，不参与逻辑 |

### 6.5 Schema

- 输入 schema 用 zod 定义，由 core 转换为 JSON Schema（zod v4 内置 `z.toJSONSchema`）。
- 同时支持直接传入 JSON Schema，不强制依赖 zod。

### 6.6 四种声明方式（原生嵌入，不解析页面）

与 chrome-devtools 这类"从外部读取整棵 DOM / 无障碍树再让模型猜"的方式不同，本库由开发者在页面内**声明**能力，
模型拿到的是简短的业务动作与结构化结果。按接入成本从低到高提供四种方式，可在同一项目中混用：

| 方式 | 包 | 写法 | 适合 |
|---|---|---|---|
| HTML 属性 | `@app-mcp/dom` | `<button data-mcp-tool="cart.clear" data-mcp-desc="清空购物车">`；`<form data-mcp-tool>` 的字段自动推导参数；列表用 `data-mcp-key` 合并为一个带 `key` 参数的工具；同时识别 WebMCP 声明式表单属性 `toolname` / `tooldescription` / `toolparamdescription` / `toolautosubmit`（6.7） | 简单按钮与表单、老项目、不写 JS |
| 状态库 | `@app-mcp/store` | `exposeZustand` / `exposeRedux` / `exposePinia`：action → 工具，state 切片 → 资源（Redux 错误类别的传递见 11.8） | 状态集中管理的项目 |
| Hook / API | `@app-mcp/react`、`@app-mcp/web` | `useTool(name, { input, handler })` | 复杂业务逻辑、自定义结果 |
| 编译期注释 | `@app-mcp/build` | `/** @mcp 结算 @risk payment */ export function checkout(addressId: string)`，参数 schema 由 TS 类型推导 | 追求零运行时改动的新项目 |

**精简快照**：`@app-mcp/dom` 提供 `ui.snapshot` 资源，只列出已声明的工具与资源及其当前状态（可用 / 不可用及原因），
一行一个，不含布局节点；页面变化时通过资源更新通知，不重复整页内容。

### 6.7 WebMCP 兼容

[WebMCP](https://webmachinelearning.github.io/webmcp/) 是 W3C Web Machine Learning 社区组起草的标准（Draft Community Group Report，
本文依据 2026-09-29 版），让页面把工具注册给**浏览器内置的 AI**；Chrome 自 149 起以 Origin Trial 提供（本地开发可开
`chrome://flags/#enable-webmcp-testing`）。本项目解决的是另一段链路——把页面工具交给**本机的 MCP 客户端**——两者互补，
`@app-mcp/web` 通过子入口 `@app-mcp/web/webmcp` 成为 WebMCP 命令式 API 的超集。

**当前规范要点**（早期文章与第三方库中的写法很多已过时）：

| 项目 | 当前规范 | 历史变化 |
|---|---|---|
| 入口 | `document.modelContext`（仅安全上下文） | 2026-05-27 由 `navigator.modelContext` 移入 `document`；Chrome 保留过渡期别名 |
| 注册 | `registerTool(tool, { signal, exposedTo })` 返回 `Promise<undefined>`；重名、非法名、空描述以 `InvalidStateError` 拒绝 | — |
| 注销 | abort 注册时传入的 `signal` | `unregisterTool(name)`、`provideContext({ tools })`、`clearContext()` 已移除 |
| 工具定义 | `name`、`title`、`description`、`inputSchema`、`execute(input, { signal })`、`annotations`（`readOnlyHint`、`consequentialHint`、`untrustedContentHint` 等） | `execute(input, client)` 与 `client.requestUserInteraction()` 已移除 |
| 跨源 | `exposedTo`：允许哪些源的嵌入方 / 智能体看到该工具；`getTools({ fromOrigins })` | — |
| 调用方接口 | `getTools()`、`executeTool(tool, args, { signal })`；事件 `toolchange`、`toolactivated`、`toolcancel` | — |
| 声明式表单 | `<form toolname tooldescription toolautosubmit>`，字段上 `toolparamdescription`；`SubmitEvent.agentInvoked` 区分智能体提交，`SubmitEvent.respondWith(promise)` 把结果交给智能体（替代默认导航）；CSS 伪类 `:tool-form-active`、`:tool-submit-active` | — |

**两种模式**（`installWebMcp(appMcp)` 自动选择）：

| 模式 | 条件 | 行为 |
|---|---|---|
| polyfill | 浏览器没有原生 WebMCP | 在 `document.modelContext`（并在 `navigator.modelContext` 放同一对象的别名）安装标准兼容对象：`registerTool` / `getTools` / `executeTool` 与三种事件 |
| 桥接 | 浏览器有原生 WebMCP（或页面已装 `@mcp-b/global` 等 polyfill） | 不替换原生对象，只覆盖其 `registerTool`：页面注册的工具同时进入原生（浏览器内置 AI 可用）与 app-mcp（Host 可用），`signal` abort 时两边同步注销；方法无法覆盖时退化为 `native-readonly` |

**双向镜像**：
- 标准 → app-mcp：按标准写的代码（以及 `@mcp-b/global`、`webmcp-react`、`use-webmcp-tool` 等库）无需改动，工具自动出现在 Host 中。
  annotations 映射为风险：`readOnlyHint` → `read`，`consequentialHint` / `destructiveHint` → `destructive`，其余 → `write`。
- app-mcp → 标准：`appMcp.tool()` / `useTool` 注册的工具也注册到原生 `modelContext`（`mirrorOwnTools`，默认开启），风险反向映射为 annotations。
- 同名冲突时 `appMcp.tool()` 优先；Host 调用经标准接口注册的工具时同样派发 `toolactivated` / `toolcancel`，行为与浏览器内置 AI 调用一致。
- 已移除的旧接口（`unregisterTool`、`provideContext` / `clearContext`、`requestUserInteraction`、旧版 MCP-B 返回值上的 `unregister()`）继续兼容，便于迁移存量代码。

**安全**：经 Host 调用时风险确认由 Host 按 `risk` 执行（第 14 节）；浏览器内置 AI 调用时由浏览器负责确认。`exposedTo` 只影响浏览器侧，
Host 侧访问控制由配对与 Host 配置决定。详细映射规则见 `packages/web/README.md`。

## 7. SDK 与 Host 通信协议

SDK 与 Host 之间使用基于 WebSocket 的 JSON-RPC 2.0 消息。这是内部协议，Host 对模型一侧才是标准 MCP。

### 7.1 握手

```jsonc
// SDK → Host
{ "jsonrpc": "2.0", "id": 1, "method": "app/hello",
  "params": { "appId": "shop", "appName": "示例商城", "origin": "http://localhost:5173",
              "token": "<配对 token，首次为空>", "sdkVersion": "0.1.0",
              "protocolVersion": "1",
              "clientKind": "web",            // web | native | hybrid
              "instanceId": "<实例 ID：每个标签页或进程唯一>",
              "appVersion": "1.2.0",
              "launchToken": "<一次性唤醒 token，仅由 Host 唤醒时存在>" } }

// Host → SDK
{ "jsonrpc": "2.0", "id": 1, "result": { "status": "paired", "token": "<新 token>" } }
// 或 { "status": "pending" }：等待用户在 Host 端确认配对
```

### 7.2 消息列表

| 方向 | 方法 | 说明 |
|---|---|---|
| SDK → Host | `app/hello` | 握手与配对 |
| SDK → Host | `tools/sync` | 全量同步当前工具列表（连接或重连时） |
| SDK → Host | `tools/changed` | 增量变更：新增、移除、启用状态变化 |
| SDK → Host | `resources/sync`、`resources/changed` | 资源列表同步与变更 |
| SDK → Host | `resources/updated` | 已订阅资源的内容变化（节流、可带差量） |
| Host → SDK | `tools/invoke` | 调用工具，带 `callId`、参数、超时 |
| Host → SDK | `tools/cancel` | 取消进行中的调用 |
| Host → SDK | `resources/read` | 读取资源 |
| Host → SDK | `resources/subscribe`、`unsubscribe` | 订阅管理 |
| SDK → Host | `app/ready` | 唤醒后初始化完成、动态工具已注册，Host 可发送排队中的调用 |
| Host → SDK | `app/activate` | 请求已运行的 App 切换到前台或后台状态 |
| SDK → Host | `app/visibility` | 实例状态变化：前台 / 后台 / 冻结 / 恢复，以及是否获得焦点（用于实例路由与心跳判断） |
| 双向 | `ping` | 心跳 |

### 7.3 调用结果

```jsonc
{ "jsonrpc": "2.0", "id": 42,
  "result": { "ok": true, "data": { "orderId": "A1024", "total": 199 },
              "stateHints": ["cart.state"] } }  // 提示模型哪些资源已变化
```

错误分类：`TOOL_NOT_FOUND`、`TOOL_DISABLED`、`INVALID_INPUT`、`USER_REJECTED`、`TIMEOUT`、`HANDLER_ERROR`、`APP_DISCONNECTED`、`APP_NOT_INSTALLED`、`LAUNCH_FAILED`、`APP_NOT_RESPONDING`（UI 线程卡住）、`INSTANCE_FROZEN`（后台页面被冻结且无法激活）。错误信息要对模型友好，说明原因和建议的下一步。

### 7.4 规范化与版本

- 本协议作为**正式规范**维护（`spec/protocol.md`），各语言 SDK 都是它的实现；消息类型由 `app-mcp-protocol` crate 定义，并导出 JSON Schema。
- 握手时双方交换 `protocolVersion`，Host 选择双方都支持的最高版本；不兼容时返回明确错误，提示升级 SDK 或 Host。
- 除 WebSocket 外，还支持命名管道（Windows）、Unix socket（macOS / Linux）、浏览器扩展 Native Messaging、宿主 IPC 桥接，消息内容不变（选择规则见 12.5）。
- `app-mcp-conformance` 提供一致性测试（见 11.6），任何 SDK 通过即视为符合规范。

## 8. Host 设计

### 8.1 职责

- 用 Rust 实现，基于官方 Rust SDK `rmcp`，以单个可执行文件分发，与客户端核心共用 `app-mcp-protocol`。
- 以 stdio 方式（后续支持 Streamable HTTP）作为 MCP Server 被客户端启动。
- 监听 `127.0.0.1:7717` 接受 SDK 连接。
- 聚合所有已连接 App 的工具，名称格式为 `<appId>.<toolName>`。
- 任一 App 工具变化时发送 `notifications/tools/list_changed`。
- 执行策略检查、确认弹窗、审计记录。
- 提供内置工具：`apps.list`（列出已安装 / 已连接 App、运行状态及能力级别）、`batch`（一次执行多个调用）、L2 系统工具。
- 维护**静态清单注册表**：未运行 App 的静态工具同样出现在工具列表中。
- 负责**唤醒与回连**：调用未运行 App 的工具时，按清单唤醒 App、排队等待回连，再转发调用（见第 10 节）。
- 在各平台注册统一的 `appmcp://` 协议。
- 按**实例**管理连接：同一 App 的多个标签页或进程各自登记，调用按 12.2.2 的规则路由；提供 `apps.select({ appId, instanceId })` 让模型指定后续调用的目标实例。
- 同时监听 WebSocket、本地 socket，并作为浏览器扩展的 Native Messaging 宿主。

### 8.2 骨架代码（示意）

工具列表是动态的，因此手动实现 `ServerHandler` 的 `list_tools` 与 `call_tool`，不使用 `#[tool]` 宏。类型名与方法签名以 `rmcp` 当前版本为准。

```rust
use std::sync::Arc;
use rmcp::{ServerHandler, ServiceExt, RoleServer, ErrorData as McpError,
           model::*, service::RequestContext, transport::stdio};

#[derive(Clone)]
struct Host {
    registry: Arc<Registry>,   // 已连接 App 的动态工具 + 静态清单工具 + 内置工具
    policy: Arc<Policy>,       // 风险分级、原生确认框、限流
    audit: Arc<Audit>,
}

impl ServerHandler for Host {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools().enable_tool_list_changed()
                .enable_resources().enable_resources_subscribe()
                .build(),
        )
    }

    async fn list_tools(&self, _req: Option<PaginatedRequestParams>, _cx: RequestContext<RoleServer>)
        -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(self.registry.all_tools()))
    }

    async fn call_tool(&self, req: CallToolRequestParams, _cx: RequestContext<RoleServer>)
        -> Result<CallToolResult, McpError> {
        let target = self.registry.resolve(&req.name)?;
        self.policy.check(&target, req.arguments.as_ref()).await?;  // 风险分级与确认
        let result = target.invoke(req.arguments).await?;           // 已连接直接调用，否则唤醒 + 回连
        self.audit.record(&target, &result);
        Ok(result.into_mcp())
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let host = Host::load().await?;
    tokio::spawn(host.clone().accept_apps("127.0.0.1:7717"));   // SDK 连接入口

    let service = host.clone().serve(stdio()).await?;
    let peer = service.peer().clone();
    host.registry.on_tools_changed(move || {
        let peer = peer.clone();
        tokio::spawn(async move { let _ = peer.notify_tool_list_changed().await; });
    });
    service.waiting().await?;
    Ok(())
}
```

### 8.3 客户端配置示例

推荐：Host 作为当前用户的常驻服务运行（8.4），客户端直接连它的 Streamable HTTP 端点：

```json
{
  "mcpServers": {
    "app-mcp": { "type": "http", "url": "http://127.0.0.1:7718/mcp" }
  }
}
```

Claude Code：`claude mcp add --transport http app-mcp http://127.0.0.1:7718/mcp`。
需要令牌时（多用户机器，`--auth all`）加 `"headers": {"Authorization": "Bearer ${APP_MCP_TOKEN:-}"}`，令牌放环境变量。

仍支持 stdio（`{"command": "app-mcp-host", "args": ["stdio"]}`），仅用于测试 / 单客户端 / 无法安装服务的环境。

Host 通过各平台安装包、`cargo install`，以及 npm 包装器（`npx @app-mcp/host`，内含预编译二进制）分发；
安装后执行 `app-mcp-host service install` 注册登录自启。

### 8.4 多客户端问题（已实现：常驻服务）

问题根源：stdio 模式下每个 MCP 客户端各启动一个 Host 进程，而 App 连接端口（WebSocket 7717）只能被一个进程监听。

方案：**一个常驻 Host 进程原生服务多个 MCP 会话**，不做 stdio 转发 / 代理进程。

- `app-mcp-host serve`：同一进程提供 App 连接服务（7717）与 MCP Streamable HTTP（`127.0.0.1:7718/mcp`）。
  每个客户端一个 HTTP 会话（`Mcp-Session-Id`），共享 App 连接；`apps.select`、首次附带总览、资源订阅按会话计算（spec/hub-api.md 3.6）。
- `app-mcp-host service install|uninstall|status|start|stop`：当前用户的登录自启，无需管理员——
  Linux systemd `--user` unit、macOS launchd LaunchAgent、Windows `HKCU\…\Run` 项（指向无控制台窗口的 `app-mcp-hostw.exe`）。
  服务文件由代码生成，启动命令为 `<exe> serve --home <配置目录>`。
- 配置 `~/.app-mcp/config.json`（清单、上游、端口、生命周期、日志），命令行覆盖配置；日志写 `~/.app-mcp/logs/`（按大小轮转）。
- 单实例：端口被占用时探测 `GET /healthz`（服务名 + 版本 + pid）；是健康的 app-mcp → 退出码 0，其他程序 → 报错。
- 安全：只绑回环 + Host / Origin 校验 + 本地访问令牌 `~/.app-mcp/token`（0600）：浏览器来源（带 Origin）强制，
  本地客户端默认不强制、可配置为强制（`--auth all`）。理由见 `crates/host/README.md`「安全」。
- stdio 模式保留，但不再是推荐接入方式。

实现：`crates/host`（命令行、配置、服务安装、探测、日志）+ `crates/hub`（`serve_http_with`、`/healthz`、令牌）。

### 8.5 App 总览：首次接触时附带

工具多了之后，模型只看工具描述很难把握一个 App 的整体能力和正确用法。每个 App 可以提供一份**总览**，
Host 在模型**第一次接触**该 App 时附带给它。总览是给模型读的说明文字，不生成 Skill 或任何文件。
完整规则见 `spec/protocol.md` 第 7 节。

| 项目 | 设计 |
|---|---|
| 内容 | `summary`：一句话简介（≤ 100 字符）；`body`：Markdown 正文（≤ 2000 字符），建议小节为适用场景、能力范围、典型流程、前置条件、不支持的操作、风险说明 |
| 来源 | SDK 握手时携带（优先）；静态清单中的 `overview`（App 未运行时使用）；网页由 `@app-mcp/build` 从 `overview.md` 生成 |
| 会话开始 | MCP `initialize` 的 `instructions` 列出每个已知 App 的一句话简介 |
| 首次接触 | 会话中第一次返回某 App 的工具调用结果时，在结果最前面附带完整总览；同一版本不再重复，版本（内容哈希）变化后再附带一次 |
| 随时查看 | `apps.overview({ appId })` 返回完整总览（上下文压缩后可重新获取）；`apps.list` 包含简介 |
| 长度控制 | Host 强制截断，App 再多，默认常驻上下文的也只有每个 App 一行简介 |
| 静态与动态分开 | 总览只写长期不变的能力；当前页面、登录状态、当前可用工具等放在资源里 |
| 安全 | 注入时标明来源并用 `<app-overview>` 包裹；总览只描述、不授权，风险确认始终由 Host 按工具风险等级执行 |

实施：协议与网页 SDK、Host、构建插件在 M1 完成；原生 SDK（C、C#、Kotlin、Swift、Python、Dart、Node）在集成阶段补上 `overview` 配置项。

### 8.6 注册为 Windows 智能体连接器

> 依据微软 Learn 文档（2026-09-30 核实，均标注为预发布内容）：
> [MCP on Windows 概述](https://learn.microsoft.com/en-us/windows/ai/mcp/overview)、
> [注册 MCP 服务器](https://learn.microsoft.com/en-us/windows/ai/mcp/servers/mcp-server-overview)、
> [包身份注册](https://learn.microsoft.com/en-us/windows/ai/mcp/servers/mcp-windows-identity)、
> [隔离](https://learn.microsoft.com/en-us/windows/ai/mcp/servers/mcp-containment)、
> [MCP bundle](https://learn.microsoft.com/en-us/windows/ai/mcp/servers/mcp-mcpb)、
> [手动注册](https://learn.microsoft.com/en-us/windows/ai/mcp/servers/mcp-manual)、
> [odr.exe](https://learn.microsoft.com/en-us/windows/ai/mcp/odr-tool)。

**Windows 侧机制**：

| 项目 | 现状 |
|---|---|
| 名称 | **Windows On-device Agent Registry（ODR）**：系统级 MCP 服务器注册表；登记其中、供智能体使用的 MCP 服务器称为 **agent connector（智能体连接器）** |
| 系统要求 | Windows build 26220.7262 或更高；公开预览 |
| 使用方 | VS / VS Code 的 GitHub Copilot agent mode、Microsoft Agent Framework 等；系统自带 File Explorer、Windows Settings 连接器。宿主通过 `odr.exe list`（JSON）发现服务器，按其中的 `mcp_config` 以 **stdio** 启动并连接。宿主应具备包身份（预览版暂不强制） |
| 注册方式 | ① **MSIX / 外部位置打包**（有包身份）：包清单中 `uap3:AppExtension Name="com.microsoft.windows.ai.mcpServer"`，`Registration` 指向一份 **MCP bundle（MCPB）`manifest.json`**，安装 / 卸载时自动注册 / 注销；② **`.mcpb` 包**（无包身份）；③ **手动**：`odr mcp add <manifest.json>`、远程服务器 `odr mcp add --uri <url>`；另有 `odr mcp list / remove / configure / run` |
| 静态声明 | MCPB `_meta["com.microsoft.windows"].static_responses` 必须写出 `initialize` 与 `tools/list` 的完整响应，且与运行时**完全一致**；**服务器不得动态改变工具集合、描述或 schema**，否则不能以默认（隔离）模式运行 |
| 隔离 | 默认在**独立的 Windows 会话、独立的智能体用户账户**中运行（agent session）；不能直接访问用户会话中的文件、设置 / 注册表 / 凭据、**用户正在使用的应用与窗口**，也不能运行修改用户会话的程序；可访问网络与智能体自己的文件。用户文件需在包清单中声明 `documentsLibrary` 等能力，使用时提示用户授权，授权按**宿主**生效而非按服务器 |
| 隔离的前提 | 必须是 `.exe`、有包身份并经 MSIX 扩展注册、带合规的 MCPB 清单。`.mcpb` 与未打包的服务器无法隔离，默认**不会**出现在 ODR 中，除非用户在"设置 > 系统 > 高级 > AI 组件"打开 *Reduce protections for agent connectors*（仅建议测试用） |
| 管理 | 用户与 IT 管理员可在"设置"与 Intune 中按智能体控制访问；交互可记录审计 |
| 相邻框架 | **App Actions on Windows**：App 以动作 JSON + 包清单注册的原子动作，输入输出为实体（Document、Photo、Text 等），经 URI 激活或 COM `IActionProvider` 调用，不是 MCP；**Agent Launchers**：基于 App Actions、登记在 ODR 中的"可对话智能体"入口 |

**对本项目的影响**：

1. **动态工具与静态声明冲突**：Host 的工具列表随 App 连接、路由、界面状态变化，而隔离模式要求工具集合固定。
   因此不能把"全量动态工具"的 Host 直接登记为隔离连接器。可行做法是登记一个**工具集合固定的网关连接器**
   （如 `apps.list`、`apps.overview`、`apps.tools`、`apps.call({ tool, arguments })`），由模型经网关间接发现和调用各 App 的工具；
   风险确认仍由 Hub 按目标工具的 `risk` 执行。
2. **会话隔离**：隔离的连接器进程在智能体会话中运行，看不到用户会话中的窗口，也不能启动用户会话中的程序，
   所以网关只能做**转发**：连接用户会话中常驻的 Host（`app-mcp-host serve`，8.4），由它负责 App 连接与唤醒。
   跨会话的本地通道（回环 TCP、命名管道 ACL）是否可用，文档未说明，需实测。
3. **远程注册**：`odr mcp add --uri` 可把常驻 Host 的 Streamable HTTP 端点（`serve`）登记为"远程"服务器，是另一条候选路径；
   其工具是否同样受静态声明约束、是否经过隔离，文档未说明，需实测。
4. **反向接入**：ODR 中其他 App 的连接器本身是 MCP 服务器，可从 `odr.exe list` 读取启动命令，作为上游（`--upstream`）聚合进 Hub。
5. **代码生成**：`app-mcp-codegen` 可为 App 生成 MCPB 清单（含 `static_responses`），只收录清单中的静态工具；也可生成 App Actions 动作 JSON（M5）。
6. **安全**：ODR 的用户 / 管理员授权与 Host 的风险确认互相独立，两者都保留；总览文字不产生授权效果。

## 9. OS 能力分级

| 级别 | 对象 | 实现 | 特点 |
|---|---|---|---|
| **L1 语义工具** | 接入 SDK 的 Web / 桌面 App | 本库 SDK | 最快、最稳、有业务语义 |
| **L2 系统能力** | 文件、剪贴板、窗口、进程、通知 | Host 内置工具 | 快，需严格权限控制 |
| **L3 无障碍树** | 未接入的第三方 App | Windows UI Automation / macOS AXUIElement / Linux AT-SPI | 能拿到控件名称和层级，无需截图 |
| **L4 视觉** | Canvas、游戏等无障碍信息为空的界面 | 复用现有 computer use 方案 | 最慢，本项目不自研 |

### 9.1 L2 工具清单（初版）

| 工具 | 风险 |
|---|---|
| `os.apps.list` / `os.app.launch` / `os.app.quit` | read / write / destructive |
| `os.window.list` / `os.window.focus` / `os.window.move` | read / write / write |
| `os.clipboard.read` / `os.clipboard.write` | os-sensitive / write |
| `os.fs.list` / `os.fs.read` / `os.fs.write` | read / os-sensitive / destructive |
| `os.notify` | write |

文件系统工具默认只允许访问用户配置的白名单目录。

### 9.2 L3 工具设计

L3 不把每个控件都暴露成工具，而是提供少量通用工具：

- `ax.snapshot({ app, depth })`：返回精简后的控件树（名称、角色、可用动作、稳定 ID）。
- `ax.act({ app, elementId, action, value })`：执行点击、输入、选择等动作。
- `ax.find({ app, query })`：按名称、角色查找控件。

### 9.3 平台优先级

开发环境为 WSL2，主力桌面预计是 Windows，因此 L3 先做 **Windows UI Automation**：

- 由 `app-mcp-os-windows` crate 通过 `windows` crate 调用 UIA COM 接口，默认编译进 Host。
- UIA 调用可能被无响应的目标 App 卡住，因此 OS 能力也支持**子进程模式**：Host 通过 stdio 与之通信，崩溃或卡死时可单独重启。两种模式共用同一套代码。
- macOS、Linux 分别由 `app-mcp-os-macos`、`app-mcp-os-linux` 实现，结构相同。

注意：WSL 内的 Linux 进程无法直接访问 Windows 桌面。操作 Windows 时，Host 需要编译为 Windows 目标（`x86_64-pc-windows-msvc`）并在 Windows 侧运行；可以在 WSL 中写代码，在 Windows 侧构建和运行。

## 10. 跨平台唤醒与交互

### 10.1 各平台传统的唤醒与交互方式

| 平台 | 启动 / 唤醒 | 传参打开 | 进程间调用 | 系统级意图框架 |
|---|---|---|---|---|
| **Windows** | exe 路径、`ShellExecute`、`shell:AppsFolder\<AUMID>`（MSIX 应用） | URI 协议（注册表 `HKCU\Software\Classes\<scheme>`）、文件关联、命令行参数 | COM、命名管道、单实例转发 | Jump List、App Actions on Windows、Agent Launchers、MCP on Windows（ODR 智能体连接器，见 8.6） |
| **macOS** | `open -a` / `open -b <bundleId>` | URL Scheme（`CFBundleURLTypes`）、Universal Links、文件关联 | Apple Events / AppleScript（`.sdef` 字典）、XPC | App Intents（Shortcuts、Siri、Apple Intelligence）、Services 菜单 |
| **Linux** | `.desktop` 文件、`gtk-launch` | `xdg-open` + `x-scheme-handler/<scheme>` | D-Bus（含 D-Bus 激活：调用时自动拉起未运行的 App） | `org.freedesktop.Application`（`Activate`、`Open`、`ActivateAction`）、`.desktop` Actions |
| **Android** | 显式 Intent | 隐式 Intent、Deep Link、App Links | Bound Service、AIDL、ContentProvider | App Shortcuts、AppFunctions（Android 16 起） |
| **iOS** | 不能随意启动其他 App | URL Scheme、Universal Links、x-callback-url | 基本没有 | App Intents |
| **Web / PWA** | URL | 路由参数、`protocol_handlers`、Web Share Target | `postMessage`、BroadcastChannel | WebMCP（W3C 社区组草案，Chrome 149 起 Origin Trial，见 6.7） |

> App Actions on Windows、AppFunctions、App Intents、WebMCP 均为较新的框架，具体 API 以各平台官方文档为准。

**传统方式的共同问题**：

1. **单向**：Deep Link、URI 协议只负责打开 App，调用方拿不到结果。
2. **只管启动**：App 打开后能做什么，调用方无从得知，只能继续操作界面。
3. **无法发现**：没有统一方法列出 App 的能力和参数。
4. **参数无类型**：全是字符串，没有 schema 校验和结构化错误。
5. **冷启动慢**：每次都走完整启动流程，并抢占前台焦点。

**趋势**：苹果（App Intents）、谷歌（AppFunctions）、微软（App Actions、ODR 智能体连接器）、浏览器（WebMCP）都在让 App 向 AI 暴露能力，本质与本项目一致，但各做一套（对比见第 22 节）。本项目的机会是**一次定义，同时对接 MCP 与各平台原生框架**（见 10.7）。

### 10.2 核心机制：按需唤醒 + 回连

唤醒只负责把 App 拉起来，调用与结果始终走 MCP 通道。对模型来说只有一个工具名，App 是否在运行由 Host 处理。

```
模型调用 shop.cart.checkout({ addressId })
            │
            ▼
      Host 查询 shop 的状态
   ┌────────┼──────────────────┐
 已连接   已安装但未运行          未安装
   │        │                    │
 直接调用  ① 生成一次性 launchToken  返回 APP_NOT_INSTALLED
 (毫秒级)  ② 按清单中的平台方式唤醒     并附安装指引
           ③ 调用进入等待队列
           ④ SDK 启动后携带 launchToken 回连 Host
           ⑤ SDK 注册动态工具，发送 app/ready
           ⑥ Host 转发排队中的调用，返回结果
```

**时序与超时**：

| 阶段 | 默认超时 | 超时处理 |
|---|---|---|
| 唤醒命令执行 | 5 秒 | 按清单尝试下一种唤醒方式 |
| 等待 SDK 回连 | 15 秒 | 返回 `LAUNCH_FAILED`，附已尝试的方式 |
| 等待 `app/ready` | 10 秒 | 返回 `TIMEOUT` |

**launchToken 传递方式**（按平台）：URL 查询参数、URI 协议参数、命令行参数、环境变量、D-Bus 调用参数。SDK 统一从这些位置读取。

已连接过、因空闲而**休眠**的实例走同一条链路，但使用实例上报的唤醒描述与一次性 `wakeToken`，回连时可跳过完整同步（第 21 节）。

### 10.3 静态能力清单 `app-mcp.json`

动态注册的工具只在 App 运行时可见。为了让模型在 App 未运行时也能发现能力，每个 App 提供一份静态清单，安装或首次配对时登记到 Host。

```jsonc
{
  "appId": "shop",
  "name": "示例商城",
  "version": "1.2.0",
  "launch": {                                   // 各平台唤醒方式，按顺序尝试
    "windows": [{ "type": "uri", "scheme": "shop-app" },
                { "type": "aumid", "id": "Company.Shop_xxx!App" },
                { "type": "exe", "path": "%LOCALAPPDATA%\\Shop\\Shop.exe" }],
    "macos":   [{ "type": "bundle", "id": "com.company.shop" }],
    "linux":   [{ "type": "dbus", "name": "com.company.Shop" },
                { "type": "desktop", "file": "com.company.Shop.desktop" }],
    "web":     [{ "type": "url", "href": "https://shop.example.com/" }]
  },
  "tools": [
    {
      "name": "cart.checkout",
      "description": "结算当前购物车",
      "inputSchema": { "type": "object", "properties": { "addressId": { "type": "string" } },
                       "required": ["addressId"] },
      "risk": "payment",
      "activation": "foreground"
    },
    {
      "name": "orders.search",
      "description": "搜索历史订单",
      "inputSchema": { "type": "object", "properties": { "keyword": { "type": "string" } } },
      "risk": "read",
      "activation": "headless"
    }
  ]
}
```

**两类工具**：

| 类型 | 来源 | 可见时机 | 例子 |
|---|---|---|---|
| 静态工具 | 清单 | 始终可见，调用时自动唤醒 | 搜索订单、新建文档 |
| 动态工具 | 运行时 `useTool` | 对应界面打开时才可见 | 当前购物车的"移除某商品" |

**`activation` 取值**：

| 值 | 行为 | 例子 |
|---|---|---|
| `headless` | 后台启动，不显示窗口，执行完可退出 | 搜索、导出 |
| `background` | 启动但不抢焦点 | 同步数据、添加待办 |
| `foreground` | 启动并切到前台，用户需要看到结果或确认 | 结算、打开编辑 |

**生成方式**：`@app-mcp/build` 在构建时扫描标记为 `static: true` 的工具定义，生成清单，避免手写两份。

### 10.4 统一调用链接 `appmcp://`

Host 在各平台注册统一 URI 协议：

```
appmcp://<appId>/<toolName>?args=<base64url(JSON)>&exp=<过期时间>&sig=<Host 签名>
```

用途：

- **给人用**：模型在聊天、邮件、通知中生成链接，用户点击即可执行，相当于带 schema 与签名校验的"一键深链接"。
- **兼容只认 URI 的场景**：快捷方式、脚本、系统自动化工具也能调用 MCP 工具。
- **跨设备**（后期）：手机上点击链接，经配对设备转发到桌面执行。

处理流程：系统把链接交给 Host → Host 校验签名与过期时间 → 走正常的策略检查（风险确认不因链接而跳过）→ 调用工具。

### 10.5 接入未改造的 App：能力导入器

未接入 SDK 的 App 通常已有传统接口，`app-mcp-importers` 把它们转换为工具：

| 来源 | 转换结果 | 例子 |
|---|---|---|
| URI 协议 / Deep Link | 仅打开、无返回值的工具 | `vscode://file/<path>` → `vscode.openFile({ path })` |
| macOS `.sdef` 脚本字典 | 有参数、有返回值的工具 | Finder、Mail、Music |
| Linux D-Bus introspection | 自动生成方法工具 | MPRIS 媒体播放器：`player.play` / `pause` / `next` |
| `.desktop` Actions、Jump List | 快捷动作工具 | "新建隐私窗口" |
| App Intents / AppFunctions | 由移动端伴侣 App 代为暴露 | 手机端能力 |
| 均无 | 回退到无障碍树（L3） | 通用 `ax.*` 工具 |

导入的工具标记来源和能力（是否有返回值），模型据此决定是否需要后续验证。

### 10.6 调用路径优先级

Host 对每次调用按以下顺序选择路径：

| 优先级 | 路径 | 典型耗时 | 返回结果 |
|---|---|---|---|
| 1 | 已连接 SDK 直接调用 | < 20 ms | 结构化 |
| 2 | 系统原生意图框架（App Intents / AppFunctions / App Actions / ODR 连接器） | 数十至数百毫秒 | 有 |
| 3 | 进程间调用（D-Bus、Apple Events、COM） | 数十毫秒 | 有 |
| 4 | 唤醒 + SDK 回连 | 冷启动，约 0.5–3 秒 | 结构化 |
| 5 | Deep Link / URI（只打开） | 冷启动 | 无 |
| 6 | 无障碍树（L3） | 每步数百毫秒，需多步 | 部分 |
| 7 | 截图 + 视觉（L4） | 每步数秒 | 依赖识别 |

**提速手段**：

1. **预热**：Host 统计常用 App，空闲时让其后台常驻或保持热启动，把第 4 级降为第 1 级。
2. **预测唤醒**：模型调用 `apps.list` 或读取某 App 的资源时即提前唤醒。
3. **无界面执行**：`headless` 工具在后台运行（Electron 隐藏窗口或仅主进程、安卓 Service / AppFunctions、Linux D-Bus 激活）。
4. **单实例转发**：App 已运行时，重复唤醒由现有实例接收，不启动新进程。
5. **批量调用**：`batch` 工具一次往返执行多个调用，减少模型交互轮次。

### 10.7 一次定义，多端输出

```
            useTool / 工具定义文件（唯一来源）
                          │  @app-mcp/build + app-mcp-codegen
   ┌───────────┬─────────┼───────────┬──────────────┬───────────────────────┐
 app-mcp.json   MCP 工具   App Intents   AppFunctions   D-Bus 接口 / Windows App Actions
 (静态清单)               (Swift 代码)   (Kotlin 代码)  (XML / 清单)
```

同一个 App 由此可被 Claude（MCP）与系统级助手（Siri、Gemini、Copilot 等）调用，开发者无需分平台重复实现。此部分依赖各平台框架成熟度，放在 M5。

### 10.8 移动端

- iOS / Android 上无法运行常驻 Host，移动端能力通过各自的原生框架（App Intents、AppFunctions）暴露给系统助手。
- 桌面 Host 可通过移动端**伴侣 App** 与手机配对，间接调用手机 App 能力或转发 `appmcp://` 链接。
- 均放在 M5 之后评估。

## 11. 多语言与多 UI 框架策略

UI 框架有几十种（React、Vue、Svelte、Angular、Flutter、SwiftUI、Jetpack Compose、WPF、WinUI、Qt、GTK……），逐个适配不可行。本项目的策略是：**挂在业务动作层而不是 UI 层；按编程语言而不是 UI 框架提供 SDK；核心逻辑用 Rust 只实现一份，通过绑定提供给各语言。**

### 11.1 挂在命令 / 动作层

工具的本质是业务动作，与按钮用什么框架绘制无关。几乎所有 UI 体系都有独立于界面的命令 / 动作抽象，SDK 登记在这一层：

| 技术栈 | 现成的命令 / 动作层 |
|---|---|
| React / Vue / Svelte | Redux action、Pinia / Zustand store 方法 |
| WPF / WinUI / Avalonia | `ICommand`（MVVM） |
| Qt | `QAction`、信号槽 |
| macOS AppKit | `NSMenuItem` action、Responder Chain |
| SwiftUI / UIKit | ViewModel 方法、App Intents |
| Android | ViewModel、AppFunctions |
| Flutter | `Intent` / `Action` 体系、Bloc / Riverpod |
| Electron / VS Code 类应用 | 命令注册表 |

因此每个 SDK 的基础 API 都是命令式的 `tool(name, definition, handler) → handle`；框架适配（React Hook、Vue composable、SwiftUI 修饰符等）只负责"挂载时注册、卸载时注销"，每个约 50–100 行，是可选的便利层。

按语言归类后，几十个 UI 框架收敛为 **8 个 SDK**（见 5.2）。未接入的 App 由 L3 无障碍接口兜底，它按操作系统适配而不是按 UI 框架适配；有后端 API 的 App 也可以直接在后端登记工具，绕开 UI 层。

### 11.2 为什么选择 Rust 核心

| 收益 | 说明 |
|---|---|
| 行为一致 | 连接状态机、重连、调用队列、取消、资源节流等逻辑只有一份实现，各语言不会出现细微差异 |
| Host 与客户端共用代码 | `app-mcp-protocol` 同时被 Host 与各 SDK 使用，协议类型不会漂移 |
| 覆盖所有目标 | 同一份代码可编译为浏览器 WASM、Node 原生模块、iOS / Android / 桌面动态库 |
| 系统能力 | `windows`、`objc2`、`zbus` / `atspi` 等 crate 可直接调用各平台 API，OS 能力与 Host 同语言 |
| 分发简单 | Host 为单个可执行文件，无需运行时 |
| 性能与安全 | 无 GC 停顿；内存安全降低 FFI 之外的崩溃风险 |

| 代价 | 应对 |
|---|---|
| 构建与发布复杂（多目标、多包管理器） | 统一 CI 矩阵与版本同步发布脚本（11.5） |
| 浏览器端多一个 WASM 包 | 按需加载、体积预算与备选方案（11.4） |
| FFI 调试困难（线程、内存、异常跨边界） | 窄接口 + JSON 边界、panic 捕获、各绑定一致性与泄漏测试（11.3、11.6） |

### 11.3 分层结构

```
┌────────────────────────────────────────────────────────────────────────┐
│ 框架适配：React Hook、Vue composable、SwiftUI 修饰符、WPF Behavior …     │ 可选，每个 50–100 行
├────────────────────────────────────────────────────────────────────────┤
│ 语言习惯封装：Promise / async-await / 协程 / Task / Future、UI 线程调度  │ 每种语言 200–500 行
├────────────────────────────────────────────────────────────────────────┤
│ 生成的绑定：wasm-bindgen / napi-rs / uniffi / flutter_rust_bridge / cbindgen │ 自动生成
├────────────────────────────────────────────────────────────────────────┤
│ Rust 核心：app-mcp-core + app-mcp-protocol + app-mcp-transport          │ 唯一实现
└────────────────────────────────────────────────────────────────────────┘
```

### 11.4 核心设计要点

**sans-IO 核心**

`app-mcp-core` 本身不做任何 I/O，也不依赖具体异步运行时，只是一个状态机：输入"收到的消息""定时器到期""注册 / 注销请求"，输出"要发送的消息""要调用的 handler""状态变化事件"。I/O 与定时器由 `app-mcp-transport` 按目标平台提供：

| 目标 | 运行时 | 传输 |
|---|---|---|
| 浏览器（wasm32） | 单线程，wasm-bindgen-futures | 浏览器原生 WebSocket（web-sys） |
| 原生（桌面、移动、Node） | 核心自带的后台线程（tokio current-thread） | tokio-tungstenite、命名管道、Unix socket |

这样浏览器端不需要 tokio，WASM 体积更小，同一套逻辑也能在所有平台上做确定性测试。

**窄 FFI 接口**

对外只暴露十个左右的接口，参数和返回值尽量用 JSON 字符串传递，避免每种绑定都要映射复杂类型：

```rust
// 示意：各绑定共享的对外接口
pub struct Client;          // new(config_json) / connect() / close() / on_state(listener)
pub struct Scope;           // client.scope(name) → Scope；dispose() 注销其下全部工具与资源
pub struct ToolHandle;      // set_enabled(bool) / dispose()
pub struct ResourceHandle;  // notify_changed() / dispose()

pub trait ToolHandler: Send + Sync {          // 由各语言实现（uniffi callback interface 等）
    async fn invoke(&self, call: CallContext, args_json: String) -> Result<String, ToolError>;
}
pub trait ResourceReader: Send + Sync {
    async fn read(&self) -> Result<String, ToolError>;
}
```

各语言的类型安全由 `app-mcp-codegen` 从清单生成的类型化接口提供（11.7），不依赖 FFI 层的复杂类型映射。

**线程模型**

UI handler 通常必须在 UI 线程执行。核心在后台线程收到调用后，由语言封装层切换到正确的线程：

| 平台 | 切换方式 |
|---|---|
| Web | 本身单线程，无需切换 |
| WPF / WinUI | `Dispatcher` / `DispatcherQueue` |
| Swift | `@MainActor` |
| Android / Kotlin | `Dispatchers.Main` |
| Flutter | 回到主 isolate |
| Qt | `QMetaObject::invokeMethod`（queued connection） |

工具定义可以声明 `thread: "ui" | "any"`，不涉及界面的 handler 可以直接在后台线程执行。

**异步与取消**

- 每次调用携带取消令牌；收到 `tools/cancel` 或超时时触发，映射为各语言的 `AbortSignal`、`CancellationToken`、`Task` 取消、协程取消。
- handler 抛出的异常在封装层捕获，转换为 `HANDLER_ERROR`，不跨越 FFI 边界。
- Rust 侧所有 FFI 入口捕获 panic，转换为错误返回，避免宿主进程崩溃。

**生命周期与内存**

- 所有 handle 支持显式 `dispose()`，并以析构（Rust `Drop`、C# finalizer、Swift `deinit`、Kotlin `Cleaner`）作为兜底。
- 核心对回调持有强引用，由 handle 的 `dispose()` 释放；语言封装层文档说明如何避免循环引用（如 Swift 闭包中使用 `[weak self]`）。

**错误模型**

Rust 侧定义统一错误枚举，错误码与协议一致（7.3），各语言映射为自己的异常或 `Result` 类型。

**体积预算与备选方案**

| 产物 | 预算 | 手段 |
|---|---|---|
| 浏览器 WASM 核心 | gzip 后 < 150 KB（M1 验证） | 不引入 tokio；客户端不做 JSON Schema 校验（由 Host 负责）；`opt-level = "z"`、LTO、`wasm-opt`；按需加载 |
| 各平台原生库 | < 3 MB | LTO、strip、按平台裁剪特性 |

如果 M1 验证 WASM 超出预算且无法优化，Web 端改用纯 TypeScript 实现的客户端，以一致性测试（11.6）保证行为一致；其他语言仍使用 Rust 核心。

### 11.5 版本与发布

- 核心与所有绑定**同版本号同步发布**；协议版本独立，通过握手协商（7.4）。
- CI 矩阵：
  | 目标 | 产物 |
  |---|---|
  | x86_64 / aarch64 × Windows、macOS、Linux | Host 可执行文件、原生动态库、Node 模块 |
  | wasm32-unknown-unknown | `@app-mcp/web` |
  | aarch64 / x86_64 Android | AAR |
  | iOS（真机与模拟器） | XCFramework |
- 发布渠道：crates.io、npm、NuGet、SwiftPM、Maven Central、pub.dev、PyPI，以及 Host 的各平台安装包。

### 11.6 一致性测试

`app-mcp-conformance` 提供一个可编排场景的假 Host，覆盖握手、配对、注册 / 注销、启用条件、调用、取消、超时、断线重连、资源订阅、唤醒回连等场景。

每种语言的 SDK 都附带一个很薄的测试驱动（test harness），用同一套场景运行，全部通过才能发布。这样既验证了 Rust 核心，也验证了绑定层与语言封装层（线程切换、异常转换、内存释放）。

### 11.7 类型化接口生成

跨 FFI 边界传递的是 JSON，类型安全由代码生成补上。`app-mcp-codegen` 读取清单中的 JSON Schema，为每种语言生成参数类型和 handler 接口：

```
app-mcp.json（工具名称 + JSON Schema，语言无关）
        │  app-mcp-codegen
        ├─ TypeScript：参数类型 + handler 类型
        ├─ C#：record 类型 + 接口
        ├─ Swift：Codable struct + protocol
        ├─ Kotlin：data class + interface
        ├─ Dart：class + 抽象 handler
        └─ Python：TypedDict / dataclass + Protocol
```

开发者实现生成的接口，编译器即可检查参数类型。这与 10.7 生成原生意图框架声明的机制是同一个 codegen。

### 11.8 接入注意事项

| 场景 | 注意 |
|---|---|
| 所有 SDK 的 `appId` | 只能是 `[a-z][a-z0-9-]{0,62}`（与 `spec/protocol.md`、`spec/manifest.md` 一致），且不能是保留名 `apps`、`os`、`ax`、`host`。**不支持反向域名**（`com.company.shop` 不合法）：`appId` 是工具全名 `<appId>.<tool>` 的前缀，`.` 已用作分隔符。原生开发者请把 bundle ID / 包名 / AUMID 写在清单的 `launch` 中，`appId` 另取短名（如 `shop`） |
| Redux（`exposeRedux`） | `createAsyncThunk` 中失败时应 `rejectWithValue({ kind, message, details? })`（`kind` 为协议错误类别，如 `INVALID_INPUT`），SDK 据此返回对应类别的错误；直接 `throw` 或 reject 其他值一律归为 `HANDLER_ERROR`，模型拿不到可操作的错误类别 |
| Dart / Flutter | 热重启（hot restart）会重建 Dart isolate，但原生库中的客户端与回调仍在：热重启前（或在应用重新装配时）先对旧客户端调用 `dispose()`，否则会留下悬空回调与重复实例。Dart 无法从后台线程抢占主 isolate，因此 12.3.2 的 UI 线程无响应超时在 Dart 中不可用 |

## 12. 网页与原生 App 的实现差异

网页与原生 App 共用同一套协议和 Rust 核心，差异主要来自运行环境的限制：怎么连上 Host、怎么被唤醒、怎么证明身份、生命周期怎么管理。

### 12.1 总览

| 方面 | 网页（浏览器中的 Web App） | 原生 App（桌面 / 移动） |
|---|---|---|
| 运行环境 | 浏览器沙箱，不能直接访问系统 | 普通进程，可调用系统 API |
| 连接 Host | 只能主动发起 WebSocket，受浏览器安全策略限制 | WebSocket、命名管道、Unix socket |
| 身份识别 | `Origin`，只能防住浏览器中的恶意网页 | 对端进程的 PID、路径、代码签名，更可靠 |
| 实例数量 | 同一网站可能开着多个标签页 | 通常单实例 |
| 生命周期 | 刷新、关闭标签页、后台被限流或冻结 | 进程常驻；移动端会被系统挂起 |
| 唤醒 | 只能打开新标签页，无法唤醒已打开的页面（除非有扩展） | URI 协议、D-Bus、AUMID 等多种方式 |
| 无界面执行 | 基本做不到，必须有打开的页面 | 可以后台运行 |
| 线程 | 只有主线程，handler 直接执行 | 多线程，需切回 UI 线程 |
| 登录状态 | 标签页自带 Cookie 与会话 | App 自行管理 token |
| 静态清单 | 放在网站固定地址，随部署更新 | 安装时写入，多个版本可能并存 |
| 无障碍兜底 | DOM 与 ARIA，语义丰富 | UIA、AX、AT-SPI，质量取决于 App |
| 高亮提示 | DOM 覆盖层，实现简单 | 需要原生悬浮窗 |
| 体积 | 额外下载 WASM 核心 | 原生库随 App 安装 |

### 12.2 网页端

#### 12.2.1 连接 localhost 的限制与浏览器扩展桥接

网页直连 `ws://127.0.0.1:7717` 会遇到：

- **本地网络访问限制**：Chrome 正在推行 Local Network Access，公网网站访问本机或局域网地址时需要用户授权。
- **混合内容**：HTTPS 页面连接 `ws://`。Chrome、Firefox 把 `127.0.0.1` 视为可信地址并允许连接；Safari 处理更严格，可能被拦截。
- **端口探测**：任何网站都能探测本机端口是否开放，从而得知用户安装了 Host。

> 具体行为因浏览器和版本而异，需要实测（见第 17 节浏览器兼容性测试）。

**应对**：以浏览器扩展桥接作为网页端的推荐通道，WebSocket 直连作为未安装扩展时的备选。

```
网页 SDK ──window.postMessage──> 扩展 content script ──> 扩展 service worker
                                                              │ Native Messaging（stdio）
                                                              ▼
                                                             Host
```

| 收益 | 说明 |
|---|---|
| 无 localhost 限制 | 不受本地网络访问与混合内容策略影响 |
| 不暴露端口 | 未安装扩展的用户可以关闭 WebSocket 监听 |
| 可靠的来源 | 扩展通过浏览器 API 获取标签页真实 URL，比 `Origin` 头更可信 |
| 标签页管理 | 可查找、激活已打开的页面，实现网页"唤醒" |

扩展只做消息转发与标签页管理，不包含业务逻辑，也不读取页面内容。

#### 12.2.2 多标签页与实例路由

同一网站可能开着多个标签页，每个都是一个实例：

- 握手携带 `instanceId`（每个标签页生成一次，刷新后保留在 `sessionStorage` 中）。
- Host 默认路由规则：**当前聚焦的实例 > 最近活跃的实例 > 最早连接的实例**。
- `apps.list` 列出每个 App 的所有实例（标题、URL、是否聚焦、状态）；模型可用 `apps.select` 指定目标实例，在当前会话内生效。
- 可选的**主页面模式**：同一网站的标签页通过 `BroadcastChannel` 选出一个主页面，只由它连接 Host，其他页面的调用经它转发，适合实例很多的应用。

#### 12.2.3 后台限流与冻结

浏览器会降低后台标签页的定时器频率，甚至冻结页面，导致心跳超时、被误判为断开。

- SDK 监听 `visibilitychange` 与 Page Lifecycle 的 `freeze` / `resume` 事件，通过 `app/visibility` 上报状态。
- Host 对后台网页放宽心跳超时，区分"已断开"与"已冻结"。
- 调用冻结的实例时，先通过扩展激活标签页再执行；无扩展时返回 `INSTANCE_FROZEN`，并提示用户切到该页面。

#### 12.2.4 唤醒与无界面执行

- 有扩展时：先查找已打开的标签页并激活，找不到再打开新标签页。
- 无扩展时：由 Host 调用系统默认浏览器打开 URL。
- 网页做不到真正的后台执行；需要无界面执行的能力，应在网站后端登记为服务端工具。

#### 12.2.5 静态清单

网页的清单放在网站固定地址：`https://<域名>/.well-known/app-mcp.json`。Host 首次配对时拉取，之后按 HTTP 缓存规则更新，无需安装步骤。

#### 12.2.6 登录状态

工具在标签页内执行，天然使用当前用户的 Cookie 与会话，权限与用户在页面上的操作一致，无需额外鉴权。

### 12.3 原生 App

#### 12.3.1 对端进程校验

原生 App 默认使用本地 socket，Host 可以直接获取对端进程信息：

| 平台 | 获取方式 | 校验内容 |
|---|---|---|
| Windows 命名管道 | `GetNamedPipeClientProcessId` | 进程路径、Authenticode 签名 |
| Linux Unix socket | `SO_PEERCRED` | PID、UID、可执行文件路径 |
| macOS Unix socket | `getpeereid` / audit token | UID、代码签名（Team ID、Bundle ID） |

配对时记录 App 的签名信息；之后连接时签名一致即自动放行，签名变化则重新确认。原生 App 不使用 WebSocket，从而不向网页暴露任何端口。

#### 12.3.2 UI 线程无响应

handler 大多需要在 UI 线程执行（11.4）。UI 线程卡住时调用不能无限等待：

- 语言封装层在切换到 UI 线程时设置超时（默认 10 秒）。
- 超时返回 `APP_NOT_RESPONDING`，并在审计日志中记录。

#### 12.3.3 版本与清单一致性

用户机器上的 App 版本可能落后于清单：

- 握手上报 `appVersion`。
- Host 以运行中实例实际注册的工具为准；清单中存在但未注册的工具标记为当前不可用，并在工具描述中说明原因。

#### 12.3.4 移动端

- iOS、Android 会挂起后台 App，连接随时可能中断。
- 移动端以**原生意图框架（App Intents、AppFunctions）为主通道**，SDK 直连仅在 App 处于前台时作为补充。这与桌面端正好相反。

### 12.4 混合应用（Electron、Tauri、WebView）

界面是网页、外壳是原生的应用，**由原生侧连接 Host**：

```
WebView 内页面（@app-mcp/web，IPC 桥接模式）
     │  进程内 IPC：Electron ipcRenderer / Tauri invoke / WebView2 postMessage / WKScriptMessageHandler
     ▼
原生主进程（@app-mcp/node、Rust crate、原生 SDK）
     │  命名管道 / Unix socket
     ▼
    Host
```

- 页面中 `useTool` 用法不变，只是传输方式由 WebSocket 换成进程内 IPC。
- 身份由原生进程统一证明（12.3.1），页面不需要单独配对。
- 原生菜单、托盘、文件操作等能力在主进程中登记为工具，与页面工具合并为同一个 App 的工具集。

### 12.5 传输选择规则

| 客户端类型 | 首选传输 | 备选 | 身份依据 |
|---|---|---|---|
| 混合应用中的页面 | 宿主 IPC 桥接 | — | 宿主原生进程 |
| 浏览器网页（已装扩展） | 扩展 Native Messaging | WebSocket 直连 | 扩展提供的标签页 URL + 配对 token |
| 浏览器网页（未装扩展） | WebSocket 直连 | — | `Origin` + 配对 token |
| 桌面原生 App | 命名管道 / Unix socket | WebSocket（仅调试） | 对端进程签名 + 配对 token |
| 移动原生 App | 原生意图框架 | 前台时 SDK 直连 | 系统框架 |

`@app-mcp/web` 按"宿主 IPC → 扩展 → WebSocket"的顺序自动探测；原生 SDK 默认使用本地 socket。

## 13. 零侵入与性能

| 关注点 | 做法 | 验收标准 |
|---|---|---|
| 不引起重渲染 | 注册信息存于 ref 与模块级 store，不进入框架响应式状态 | 接入前后 React Profiler 渲染次数一致 |
| 包体积 | 传输层动态 `import()`；`enabled: false` 时可 tree-shake | core + react 接入增量 < 8 KB（gzip），关闭时约 0 |
| 生命周期 | 组件挂载注册、卸载注销；路由切换自动变化 | 无泄漏（反复挂载卸载 1000 次后注册表为空） |
| 调用开销 | handler 外只有参数校验和队列调度 | 本机端到端调用 p50 < 20 ms（不含 handler 本身） |
| 状态推送 | 资源更新节流（默认 100 ms）与差量 | 高频更新时不阻塞主线程 |
| 可见性 | 可选高亮关联元素 + "AI 正在操作"提示条 | 用户可随时看到模型在做什么 |
| 人机并发 | 调用串行排队；检测到用户输入时可暂停模型调用 | 无状态竞争导致的错误 |
| 断线 | 自动重连（指数退避），重连后全量同步 | Host 重启后 5 秒内恢复 |
| 唤醒开销 | 预热、预测唤醒、单实例转发 | 热启动 App 的唤醒调用 p50 < 300 ms |

## 14. 安全模型

### 14.1 威胁

| 威胁 | 说明 |
|---|---|
| 恶意网页连接 Host | 任意网页都能尝试连接 `ws://127.0.0.1` |
| 提示词注入 | 页面或资源内容诱导模型调用危险工具 |
| 越权 | 模型执行超出当前用户权限的操作 |
| 误操作 | 模型理解错误，执行了破坏性操作 |
| 伪造唤醒链接 | URI / Deep Link 任何人都能构造 |
| 冒充被唤醒的 App | 其他进程抢先回连 Host |
| 冒充原生 App | 本机其他进程伪装成已配对的 App 连接 Host |
| 端口探测 | 网页扫描本机端口，判断用户是否安装了 Host |

### 14.2 措施

1. **本地配对**：只监听回环地址；校验 `Origin`；首次连接需用户在 Host 端确认，之后使用 token；token 可在 Host 端撤销。
2. **风险分级与确认**：
   | 等级 | 默认策略 |
   |---|---|
   | `read` | 直接执行 |
   | `write` | 直接执行，记录审计 |
   | `destructive` | Host 原生确认框 |
   | `payment` | Host 原生确认框，每次必确认，不可设为总是允许 |
   | `os-sensitive` | Host 原生确认框，支持"本次会话允许" |
3. **权限继承**：工具复用 App 原有 handler，请求仍经过后端鉴权，模型权限不超过当前登录用户。
4. **防注入**：资源和返回值中的用户生成内容标记为数据；可配置策略，如"读取外部内容后的 N 次调用内禁止高风险工具"。
5. **审计与撤销**：记录调用者、时间、工具、参数、结果；handler 可选提供 `undo`，Host 提供 `history.undo` 工具与界面。
6. **限流**：按 App、按工具限制调用频率，防止模型死循环。
7. **唤醒安全**：
   - `appmcp://` 链接必须带 Host 签名与过期时间，校验通过后仍走完整策略检查。
   - launchToken 一次性使用、短时有效，且与唤醒的目标 App 绑定。
   - 唤醒本身可能有副作用（自动登录、开始同步），纳入风险分级，可配置需确认。
   - `headless` 调用在系统托盘或通知中留下记录，保证后台操作可见。
8. **对端进程校验**：原生 App 通过本地 socket 连接，Host 校验对端进程的路径与代码签名，签名变化时重新确认（12.3.1）。
9. **减少端口暴露**：网页优先经浏览器扩展连接；WebSocket 监听可关闭；对未配对的 Origin 不返回任何可区分的信息，直接关闭连接。

## 15. 里程碑与任务拆分

### M1 原型：跑通最小闭环

**目标**：在 Claude Code 中让模型通过 MCP 操作一个 Web Demo 页面，同时验证 Rust 核心 + WASM 路线可行。

| # | 任务 | 产出 |
|---|---|---|
| 1.1 | 初始化 Cargo workspace 与 pnpm workspace、CI（Rust、wasm、JS）、lint、测试框架 | 仓库骨架 |
| 1.2 | `app-mcp-protocol`：消息类型、serde、协议版本；导出 JSON Schema；编写 `spec/protocol.md` 初版 | 协议 crate 与规范 |
| 1.3 | `app-mcp-core`：sans-IO 状态机：注册表、scope、启用条件、调用队列与取消、握手与重连 | 核心 crate |
| 1.4 | `app-mcp-transport`：浏览器 WebSocket（wasm） | 传输 crate |
| 1.5 | `@app-mcp/web`：wasm-bindgen 绑定 + JS 封装（注册缓存、按需加载、zod 转 JSON Schema） | npm 包 |
| 1.6 | WASM 体积验证：核心 gzip 后 < 150 KB；不达标则启用 11.4 的备选方案 | 体积报告 |
| 1.7 | `@app-mcp/react`：`useTool`、`useResource`、`<ToolScope>` | npm 包 |
| 1.8 | `app-mcp-manifest` + `@app-mcp/build`：清单 schema 与校验、Vite 插件生成清单 | 清单工具链 |
| 1.9 | `app-mcp-host`：基于 `rmcp` 的 stdio MCP Server、WebSocket 服务、聚合、`list_changed`、加载静态清单 | Host 可执行文件 |
| 1.10 | Demo：Todo + 购物车页面（Vite + React） | `examples/shop` |
| 1.11 | 端到端测试：MCP 客户端 → Host → Demo 页面 | e2e 用例 |
| 1.12 | 握手携带 `instanceId`，Host 按实例路由，`apps.list` 列出实例，`apps.select` 指定目标 | 多标签页可用 |

Demo 页面运行在 `http://localhost`，避免 M1 阶段遇到本地网络访问授权与混合内容问题。

**验收**：
- 在 Claude Code 中配置 Host 后，模型能列出 Demo 工具并完成"添加 3 个待办并勾选第 2 个""把购物车里最贵的商品删掉"。
- 切换路由后工具列表随之变化。
- 同时打开两个 Demo 标签页时，调用发往当前聚焦的标签页；模型可通过 `apps.select` 切换目标。
- Demo 页面关闭时，其静态工具仍出现在工具列表中（M1 调用时返回提示，自动唤醒在 M2 实现）。
- 接入前后页面交互行为和渲染次数无变化。
- WASM 体积达标，Host 可执行文件在 Windows 与 Linux 上运行。

**规模**：约 4–5 千行（Rust 约 3–4 千行，TypeScript 约 1 千行）。

### M2 可用：安全与多框架

| # | 任务 |
|---|---|
| 2.1 | 资源订阅与节流差量推送 |
| 2.2 | 风险分级、Host 原生确认框、审计日志 |
| 2.3 | 本地配对与 token 管理、Origin 校验 |
| 2.4 | daemon + 代理模式，支持多个 MCP 客户端 |
| 2.5 | 调用高亮与"AI 正在操作"提示条 |
| 2.6 | `@app-mcp/vue` |
| 2.7 | `@app-mcp/electron`：基于 `@app-mcp/node`，主进程接入、菜单命令登记 |
| 2.8 | 文档站与接入指南 |
| 2.9 | 唤醒 + 回连 + 调用排队、launchToken、超时与降级 |
| 2.10 | `launcher`：Web URL 唤醒、Windows URI 协议与 exe 唤醒 |
| 2.11 | `appmcp://` 统一协议（Windows 优先）、链接签名与校验 |
| 2.12 | `batch` 工具 |
| 2.13 | `app-mcp-conformance` 一致性测试套件；`@app-mcp/web` 全部通过 |
| 2.14 | `@app-mcp/node`：napi-rs 绑定，通过一致性测试 |
| 2.15 | 原生传输：tokio WebSocket、命名管道、Unix socket |
| 2.16 | 原生 App 默认走本地 socket；对端进程路径与签名校验 |
| 2.17 | `@app-mcp/web` 宿主 IPC 桥接模式（先支持 Electron） |
| 2.18 | 网页可见性与冻结检测、`app/visibility`、后台心跳策略 |
| 2.19 | 网站清单 `/.well-known/app-mcp.json` 拉取与缓存 |
| 2.20 | UI 线程超时检测与 `APP_NOT_RESPONDING` |
| 2.21 | Hub SDK 阶段 1：`crates/hub` 从 Host 迁出、Host 变薄壳；审批 / 配对回调；OpenAI / Anthropic / Gemini / MCP 格式导出与 `dispatch`（第 20 节） |
| 2.22 | Hub SDK 阶段 2：`hub-c`、`hub-uniffi`、`@app-mcp/hub` 绑定 |
| 2.23 | App 端生命周期 阶段 A：协议 `app/sleep` / `app/lease` / 快速恢复字段；核心状态机（`idle` / `on-demand`、`dormant` / `waking`、租约、`hold`、`toolsHash`）；原生运行时休眠释放线程（第 21 节） |
| 2.24 | 生命周期 阶段 B：Web 驱动（可见性、bfcache、URL 令牌）；C / uniffi / Node 绑定透传；各平台唤醒入口 |
| 2.25 | 生命周期 注册侧：清单 `wake` 字段（manifest、build、codegen）、惰性 handler |
| 2.26 | 生命周期 Host 配合：休眠实例、`wakeToken` 与激活、租约下发 |

**验收**：
- 安全用例全部通过（恶意 Origin 被拒、`payment` 必须确认、未配对无法调用、伪造 `appmcp://` 链接被拒）。
- Vue 与 Electron Demo 可用。
- 关闭 Electron Demo 后，模型调用其静态工具能自动唤醒并返回结果。
- `@app-mcp/web` 与 `@app-mcp/node` 通过全部一致性测试。
- Electron Demo 的页面经 IPC 桥接接入，无需单独配对；伪造签名的本地进程无法连接。
- 厂商示例：一个自有 LLM 循环的 Agent 通过 Hub SDK（任一语言绑定）以 OpenAI 或 Anthropic 格式列出并调用 Demo 工具，审批回调生效。
- 生命周期：`idle` 模式的 App 空闲后进入 `dormant`，休眠态无 socket、无定时器、原生运行时线程已释放；调用其工具时被唤醒，经快速恢复（跳过 `tools/sync`）完成调用后再次休眠。

**规模**：累计约 1 万行。

### M3 OS 层（Windows 优先）

| # | 任务 |
|---|---|
| 3.1 | `app-mcp-os-windows`：编译进 Host，并支持子进程模式 |
| 3.2 | L2 工具：窗口、应用、剪贴板、文件（白名单）、通知 |
| 3.3 | L3：UIA 控件树快照、查找、动作 |
| 3.4 | `apps.list` 返回每个 App 的能力级别 |
| 3.5 | `tauri-plugin-app-mcp`（直接依赖 Rust 核心） |
| 3.6 | `launcher`：Windows AUMID、macOS bundle、Linux D-Bus / desktop 唤醒 |
| 3.7 | `importers`：URI 协议导入、Linux D-Bus introspection 导入 |
| 3.8 | 预热与预测唤醒、`headless` / `background` / `foreground` 激活模式 |
| 3.9 | C# 绑定（技术验证 uniffi-bindgen-cs 与 C ABI + P/Invoke 后二选一）、NuGet 包、WPF Demo |
| 3.10 | `@app-mcp/extension`（Chrome / Edge）：Native Messaging 桥接、标签页查找与激活、网页唤醒 |
| 3.11 | `@app-mcp/web` 宿主 IPC 桥接扩展到 Tauri 与 WebView2 |
| 3.12 | Windows ODR：工具集合固定的网关连接器（MSIX + MCPB 清单）、跨会话连接 daemon 的实测、`odr mcp add --uri` 远程注册实测；ODR 连接器作为上游导入（8.6） |

**验收**：
- 模型能在 Windows 上完成"打开记事本，写入剪贴板内容并保存到指定目录"，全程不使用截图。
- C# SDK 通过一致性测试，WPF Demo 可被模型操作。
- 安装扩展后，HTTPS 线上网页无需直连 localhost 即可接入；调用已关闭的网页工具时，能激活已有标签页或打开新标签页。

**规模**：累计约 2 万行。

### M4 生态

| # | 任务 |
|---|---|
| 4.1 | `app-mcp-os-macos`（AX）与 `app-mcp-os-linux`（AT-SPI） |
| 4.2 | WebMCP：跟进规范变化（`@app-mcp/web/webmcp` 已在 M1 提前完成，6.7）；浏览器扩展中调用浏览器内置 AI 注册的工具 |
| 4.3 | DevTools 浏览器扩展面板 |
| 4.4 | 调用录制与回放、撤销 |
| 4.5 | Svelte 等更多框架适配 |
| 4.6 | `importers`：macOS `.sdef` 导入、`.desktop` Actions 与 Jump List 导入 |
| 4.7 | Swift、Kotlin、Dart、Python、C 绑定与语言封装，均通过一致性测试 |
| 4.8 | `app-mcp-codegen`：从清单生成各语言类型化接口 |

**验收**：8 个 SDK 全部通过一致性测试，每种语言至少有一个可被模型操作的 Demo。

### M5 原生意图框架与移动端

| # | 任务 |
|---|---|
| 5.1 | `app-mcp-codegen`：生成 App Intents（Swift）、AppFunctions（Kotlin）声明 |
| 5.2 | 生成 Windows App Actions 动作 JSON、ODR 连接器的 MCPB 清单（含 `static_responses`）与 D-Bus 接口声明 |
| 5.3 | 调用路径接入系统原生意图框架（优先级第 2 级） |
| 5.4 | 移动端伴侣 App 与跨设备 `appmcp://` 转发（评估后决定） |
| 5.5 | 移动端以原生意图框架为主通道，SDK 直连仅在前台时作为补充（12.3.4） |

**验收**：同一份工具定义生成的原生声明可被对应平台的系统助手识别并调用。

## 16. 仓库结构与技术栈

```
tastyrice/
├── Cargo.toml                # Rust workspace
├── pnpm-workspace.yaml       # JS workspace
├── spec/
│   ├── protocol.md           # SDK ↔ Host 协议规范
│   ├── manifest.md           # 静态清单
│   ├── hub-api.md            # Hub SDK 接口（第 20 节）
│   └── lifecycle.md          # App 端生命周期（第 21 节）
├── crates/
│   ├── protocol/
│   ├── core/                 # sans-IO 客户端核心
│   ├── transport/
│   ├── manifest/
│   ├── hub/                  # Hub 库 app-mcp-hub（厂商可嵌入）
│   ├── host/                 # 可执行程序 app-mcp-host（Hub 之上的薄壳）
│   ├── launcher/
│   ├── os-windows/
│   ├── os-macos/
│   ├── os-linux/
│   ├── importers/
│   ├── codegen/
│   └── conformance/          # 一致性测试套件
├── bindings/
│   ├── wasm/                 # wasm-bindgen → @app-mcp/web
│   ├── node/                 # napi-rs → @app-mcp/node
│   ├── uniffi/               # Swift / Kotlin / Python / C#
│   ├── dart/                 # flutter_rust_bridge
│   ├── c/                    # cbindgen 头文件
│   ├── hub-c/                # Hub C ABI（app_mcp_hub.h）
│   ├── hub-uniffi/           # Hub → Kotlin / Swift / Python
│   └── hub-node/             # Hub → @app-mcp/hub
├── packages/                 # JS 包
│   ├── web/                  # WASM 绑定之上的 JS 封装
│   ├── react/
│   ├── vue/
│   ├── build/
│   ├── electron/
│   └── extension/            # 浏览器扩展（Native Messaging 桥接）
├── sdks/                     # 各语言习惯封装与发布配置
│   ├── dotnet/
│   ├── swift/
│   ├── kotlin/
│   ├── flutter/
│   └── python/
├── examples/
│   ├── shop/                 # React Demo
│   ├── notes-electron/       # Electron Demo
│   └── wpf-demo/             # WPF Demo
├── docs/
└── e2e/
```

| 方面 | 选型 |
|---|---|
| 语言 | Rust（协议、核心、Host、OS 能力）；TypeScript（JS 封装、框架适配、构建插件） |
| MCP | `rmcp`（官方 Rust SDK） |
| Rust 异步 | tokio（原生）；wasm-bindgen-futures（浏览器） |
| 序列化 | serde、serde_json；schemars（导出 JSON Schema） |
| Schema 校验 | Host 端 `jsonschema` crate；JS 端 zod（可选依赖） |
| WebSocket | tokio-tungstenite（Host 与原生）；浏览器原生 WebSocket（web-sys） |
| 绑定 | wasm-bindgen、napi-rs、uniffi、flutter_rust_bridge、cbindgen |
| OS API | `windows` crate（UIA、Win32）；`objc2` 系列（macOS）；`zbus`、`atspi`（Linux） |
| JS 工具链 | pnpm、tsup、Vitest |
| e2e | Playwright |
| 浏览器扩展 | Manifest V3、Native Messaging |
| CI | GitHub Actions 矩阵（见 11.5） |
| Demo | Vite + React；Electron；WPF |

## 17. 测试策略

| 层级 | 内容 |
|---|---|
| 单元测试 | Rust：注册表增删、启用条件、scope 生命周期、队列调度、状态机转换、协议编解码；JS：封装层与框架适配 |
| 属性测试与模糊测试 | proptest 验证协议编解码往返一致；cargo-fuzz 对协议解析器做模糊测试 |
| 一致性测试 | `app-mcp-conformance` 的同一套场景在每种语言 SDK 上运行，全部通过才能发布 |
| 绑定测试 | FFI 内存泄漏（反复创建与释放 handle）、UI 线程切换、异常与 panic 转换、取消传播 |
| 浏览器兼容性测试 | Chrome、Edge、Firefox、Safari 下：HTTP / HTTPS 页面直连 localhost、本地网络访问授权、扩展桥接、后台限流与冻结 |
| 多实例测试 | 多标签页路由规则、`apps.select`、标签页刷新后 `instanceId` 保持、主页面模式切换 |
| 集成测试 | SDK ↔ Host 真实 WebSocket 连接；断线重连；全量与增量同步 |
| 端到端测试 | 用 MCP SDK 客户端连接 Host，Playwright 打开 Demo 页面，调用工具并断言界面变化 |
| 零侵入测试 | 对比接入前后的渲染次数、DOM 快照、交互录制结果 |
| 安全测试 | 伪造 Origin、无 token 连接、绕过确认、高频调用、伪造 `appmcp://` 链接、重放 launchToken |
| 唤醒测试 | 各平台各唤醒方式的成功率与耗时；唤醒失败时的降级顺序；冷启动与热启动对比 |
| 模型评测 | 固定任务集，比较 L1 语义工具与无障碍树、视觉方式的成功率、耗时和 token 消耗 |

模型评测是证明项目价值的关键数据，M1 结束时就应该产出第一版对比结果。

## 18. 风险与应对

| 风险 | 影响 | 应对 |
|---|---|---|
| 开发者不愿为每个动作写工具定义 | 接入率低 | 提供从 handler 与 TS 类型推导 schema 的工具；L3 降级保证未接入时也可用 |
| 工具数量过多占满上下文 | 模型效果下降 | 默认不暴露；按 scope 与路由裁剪；支持工具分组与按需展开 |
| 工具粒度设计不当 | 模型调用顺序出错 | 提供复合动作（多步流程登记为一个工具）；文档给出粒度指南 |
| 提示词注入 | 安全事故 | 风险分级、Host 确认、注入后限制策略 |
| WebMCP 等标准变化（已发生：入口移到 `document`、`unregisterTool` / `provideContext` 移除） | 适配成本 | 兼容层集中在 `@app-mcp/web/webmcp`，新旧写法同时支持；跟踪规范仓库与 Chrome Origin Trial 变更 |
| Windows ODR 要求静态工具集合、连接器在隔离会话中运行 | 动态工具无法直接登记为系统连接器 | 登记固定工具集合的网关连接器，转发到用户会话中的 Host daemon；预览期实测后再定（8.6） |
| MCP 规范演进 | 兼容问题 | 跟随官方 SDK 版本，Host 层隔离 |
| WSL 与 Windows 桌面隔离 | OS 层无法开发调试 | Host 编译为 Windows 目标并在 Windows 侧运行；在 WSL 中写代码，在 Windows 侧构建和运行 |
| WASM 体积与加载时间 | 包体积或首屏受影响 | 按需加载、体积预算；超预算时 Web 端改用纯 TS 客户端（11.4） |
| FFI 复杂度（线程、内存、异常跨边界） | 崩溃难排查 | 窄接口 + JSON 边界；panic 捕获；绑定测试覆盖泄漏与线程切换 |
| 多目标构建与发布成本 | CI 慢、发布易出错 | 统一 CI 矩阵、版本同步发布脚本、构建缓存 |
| 部分绑定工具成熟度不足（如 uniffi-bindgen-cs） | C# 等 SDK 受限 | 保留 C ABI + P/Invoke 备选；M3 前先做技术验证 |
| 团队 Rust 经验 | 开发速度 | M1 控制核心范围；语言封装层可由熟悉目标语言的人负责 |
| 浏览器收紧 localhost 访问策略 | 网页直连失效 | 以浏览器扩展为推荐通道；直连仅作备选；持续跟踪浏览器兼容性测试 |
| 扩展商店审核与更新延迟 | 扩展发布受阻 | 扩展只做转发，权限最小化；提供手动安装方式 |
| 多实例路由选错目标 | 在错误的标签页执行操作 | 默认聚焦优先；高风险调用在确认框中显示目标实例的标题与 URL |
| 各平台唤醒机制差异大、行为不一致 | 唤醒失败或耗时不稳定 | 清单中配置多种唤醒方式按序降级；唤醒测试覆盖各平台 |
| 静态清单与运行时代码不一致 | 模型调用到不存在的工具 | 清单由构建插件生成；SDK 回连时校验清单版本，不一致则告警 |
| 原生意图框架 API 变化快 | codegen 维护成本高 | 放在 M5，按平台独立包，跟随官方版本 |

## 19. 待决问题

1. 项目正式名称，以及 npm scope、crates.io 名称是否可用？
2. 开源还是内部使用？许可证选择？
3. C# 绑定走 uniffi-bindgen-cs 还是 C ABI + P/Invoke？（M3 前做技术验证）
4. Host 是否需要图形界面（托盘图标、配对与确认窗口、审计查看）？用什么实现？
5. 生产环境是否默认启用，还是仅在用户主动开启时加载？
6. 是否需要支持远程（非本机）模型客户端？若支持，鉴权方案需要重新设计。
7. 统一协议名称用 `appmcp://` 还是与项目正式名称一致？
8. 静态清单由 App 安装时写入 Host 目录，还是由 Host 首次配对时拉取？
9. 移动端（伴侣 App、原生意图框架）是否纳入正式范围？
10. WASM 超出体积预算时，是否接受 Web 端使用纯 TS 客户端？
11. 最低支持的 Rust 版本（MSRV），以及各平台的最低系统版本？
12. 浏览器扩展是否作为网页端的默认必装组件，还是仅推荐安装？
13. 多实例的默认路由规则是否允许用户按 App 自定义？
14. Windows ODR 接入走"固定工具网关连接器"还是"`--uri` 远程注册"？隔离会话与用户会话之间的本地通道是否可用？（M3 实测）
15. Hub SDK 的审批默认值：嵌入厂商时是否强制要求设置 `ApprovalHandler`（至少对 `payment`）？

## 20. Hub SDK（厂商接入）

接口契约见 `spec/hub-api.md`（权威）。

### 20.1 定位

App 端 SDK 让 **App** 暴露工具；Hub SDK 让 **Agent / 助手厂商**（手机厂商语音助手、车机、PC 助手、IDE、自研 Agent 框架）
把"连接本机所有 App"的能力直接嵌进自己的产品，而不必运行独立的 `app-mcp-host` 进程、也不必走 MCP。

```
厂商 Agent（自有 LLM 循环 / 自有 UI）
   │  Hub SDK：列工具、调用、读资源、事件、审批回调、工具格式导出
   ▼
Hub（嵌入厂商进程）── WebSocket / 进程内 ──► 各 App（App 端 SDK）
   │                  └─ 子进程 ──► 上游 MCP 服务器
   └─（可选）同时以 MCP stdio / Streamable HTTP 对外提供
```

`app-mcp-host` 改为 Hub 之上的薄壳；MCP 只是 Hub 的一种"对外出口"，与格式导出并列。
MCP 出口内部也调用 `Hub::call_tool`，调用语义只有一份实现：schema 校验 → 审批 → 实例路由 → 按需唤醒 → 转发、超时、取消。

### 20.2 三种接法

| 接法 | 适合 | 做法 |
|---|---|---|
| 进程外 MCP | 已支持 MCP 的客户端（Claude Desktop、Claude Code、IDE） | 运行 `app-mcp-host`（stdio 或 `--http`），零代码 |
| 嵌入 Hub + MCP 出口 | 厂商已有 MCP 客户端实现，但希望 Hub 随产品分发、共享进程 | `Hub::start` 后 `mcp_session()` / `serve_stdio` / `serve_http` |
| 嵌入 Hub + 格式导出 | 厂商自有 LLM 循环，不想引入 MCP | `export_tools(format, filter)` 交给模型；模型返回的 tool call 交给 `dispatch(format, call)`，得到该格式的工具结果消息 |

另预留进程内 App 通道 `attach_local`：厂商自带的系统 App 与 Hub 同进程时直接交换协议消息，不走 WebSocket。

### 20.3 格式导出

| 格式 | 导出形态 | 分派 |
|---|---|---|
| `Mcp` | MCP `Tool` 列表 | `{name, arguments}` → `CallToolResult` |
| `OpenAiChat` / `OpenAiResponses` | `function` 工具 | `tool_calls` / `function_call` → `tool` / `function_call_output` 消息 |
| `Anthropic` | `{name, description, input_schema}` | `tool_use` → `tool_result`（失败带 `is_error`） |
| `Gemini` | `functionDeclarations` | `functionCall` → `functionResponse` |

- **名称编码**：OpenAI / Anthropic 的工具名只允许 `[a-zA-Z0-9_-]{1,64}`，导出名把 `.` 换成 `__`，冲突或超长时截断并追加短哈希；Hub 保存双向映射，`dispatch` 同时接受导出名与全名。
- **Schema 适配**：去掉 `$schema`；Gemini 格式剔除其不支持的关键字。
- **结果与总览**：与 MCP 出口一致，含 `stateHints`；每个厂商会话（`CallRequest.session`）首次接触某 App 时附带总览（8.5）。
- **描述增强**：风险高于 `write` 的工具在描述末尾标注风险，便于模型谨慎使用。

### 20.4 审批与配对回调

- **审批**：`ApprovalPolicy.require_at_or_above` 设定阈值；达到阈值的调用交给厂商的 `ApprovalHandler`（用厂商自己的 UI 确认），返回 `false` 或超时 → `USER_REJECTED`。默认不审批，与现有 Host 一致。
- **配对**：未知 App 首次连接时询问 `PairingHandler`（默认：拒绝非白名单 Origin、接受其他）。
- 绑定层把异步回调映射为"回调 + 完成句柄"（C：`am_hub_approval_cb` + `am_hub_approval_complete(handle, bool)`）。

### 20.5 各语言绑定

| 绑定 | 形态 | 说明 |
|---|---|---|
| Rust | `app-mcp-hub` crate，async API | 可在厂商自有 tokio 运行时中使用 |
| C ABI | `app_mcp_hub.h`，前缀 `am_hub_`，`AM_HUB_API_VERSION 1` | 复杂结构一律 JSON 字符串；字符串所有权规则与 `app_mcp.h` 相同；事件与审批回调在 Hub 分发线程上执行 → C / C++ / C# / Dart |
| uniffi | `bindings/hub-uniffi` | Kotlin（Android 厂商）、Swift、Python |
| Node | `@app-mcp/hub`（napi-rs） | Electron 助手、Node Agent 框架 |

绑定层创建 Hub 自带的 tokio 多线程运行时。验收：各语言用 App 端 SDK（或 `fake_app`）连上嵌入式 Hub，完成列工具、调用、事件、审批。

### 20.6 计划

阶段 1（M2 2.21）：`crates/hub` 迁出、Host 变薄壳、回调、格式导出，迁移后 Host 集成测试全部保持通过；阶段 2（M2 2.22）：各语言绑定。
按需唤醒随 M2（2.9–2.10、2.26）与 M3（3.6）分步接入 Hub；进程内 App 通道视厂商需求实现。

## 21. App 端生命周期

契约见 `spec/lifecycle.md`（权威，协议部分合入 `spec/protocol.md`）。

### 21.1 目标

默认"启动即连接、一直在线"会让每个 App 常驻一条连接、一个心跳定时器、原生侧一个运行时线程和一个分发线程，移动端还会阻止进程被回收。目标：

1. **任务完成即释放**：没有进行中的调用、没有有效租约时，关闭连接、停心跳、释放运行时线程。
2. **不强占进程驻留**：因唤醒而冷启动的进程可按策略退出；SDK 不持有前台服务、WakeLock、后台任务断言。
3. **可主动唤醒**：Host 需要时通过操作系统原生激活机制拉起 / 叫醒 App，回连完成调用后再次休眠。
4. **唤醒要快**：休眠前留下恢复令牌与工具摘要，回连时跳过完整同步。

### 21.2 三种模式

| 模式 | 行为 | 适合 |
|---|---|---|
| `persistent`（默认，兼容现状） | 不休眠 | 桌面常驻、开发调试 |
| `idle`（移动端封装默认） | 启动时连接；空闲超时（默认 60 秒，隐藏 / 冻结时 15 秒）后休眠；唤醒后回连，处理完再按空闲规则休眠 | 普通桌面与移动 App |
| `on-demand` | 启动时**不连接**；被唤醒或 App 调用 `connectNow()` 时连接，任务完成后经 `graceMs`（默认 10 秒）休眠 | 手机、托盘工具、无界面辅助进程 |

**空闲条件**（全部满足才开始计时）：无进行中或排队的调用与资源读取；Host 没有有效租约；Host 没有订阅本实例的资源；App 未调用 `hold()`。
进程驻留由 `residency` 决定：`keep`（只断连接）、`exit-when-idle`（仅由唤醒冷启动的进程，休眠后回调 `onIdleExit`，由 App 决定是否退出）、`exit-always`。

### 21.3 休眠、唤醒与快速恢复

```
connected ──(空闲)──► 发送 app/sleep ──accepted──► dormant ──(唤醒)──► waking → connecting → handshaking
                            └─rejected──► connected（重置计时）
```

| 环节 | 设计 |
|---|---|
| 休眠 | SDK 发 `app/sleep { reason, wake, toolsHash }`；Host 接受后返回 `resumeToken`，把实例标记为**休眠**：保留工具快照，工具不从列表消失、不发 `list_changed`；Host 有待派发调用时拒绝并给出 `retryAfterMs` |
| 租约 | Host 预计还会调用时发 `app/lease { ttlMs }`（如每次调用后 60 秒），期间不休眠；`ttlMs: 0` 取消 |
| 唤醒 | 路由到休眠实例时，Host 生成一次性 `wakeToken`（≥128 位、60 秒有效），按实例上报的 `WakeDescriptor`（没有则回退清单 `launch`）激活 App；App 回连时在 `launchToken` 中携带，Host 据此派发挂起的调用 |
| 快速恢复 | `app/hello` 携带 `resumeToken` 与 `toolsHash`；一致时 Host 返回 `toolsCurrent: true`，SDK 跳过 `tools/sync` / `resources/sync`，直接 `app/visibility` → `app/ready` |
| `toolsHash` | 规范化（键排序、无空白）后的工具与资源定义的 sha256 前 16 个十六进制字符，由核心计算，各语言一致 |

### 21.4 各平台唤醒方式

| 平台 | Host 侧唤醒 | App 侧接收 | 后台唤醒 |
|---|---|---|---|
| Windows | AUMID：`IApplicationActivationManager::ActivateApplication`；未打包应用用 URI | 单实例重定向交给已运行实例 → `handleWake` | 否（托盘 / 无窗口进程可以） |
| macOS | `open -g <scheme>://app-mcp/wake?token=` 或 Apple Event | `onOpenURL` / `NSAppleEventManager` | 是 |
| Linux | D-Bus `org.freedesktop.Application.ActivateAction("app-mcp-wake", [token])`（可自动拉起进程）；否则 URI | GApplication action / Qt D-Bus adaptor | 是 |
| Android | 显式广播 `dev.appmcp.action.WAKE` | `WakeReceiver` `goAsync()` 并以加急 WorkManager 任务回连（绕开 Android 12+ 后台启动前台服务限制） | 是 |
| iOS | URL Scheme（会把 App 带到前台） | `onOpenURL` / `scene(_:openURLContexts:)` | 否（后台调用走 App Intents） |
| Web | 打开 / 聚焦带 `#app-mcp-wake=<token>` 的 URL | 页面加载或变为可见时回连，令牌读取后从地址栏移除；bfcache 前立即休眠 | 否 |
| Electron / Tauri | 同原生桌面；主进程由 URI 拉起 | `second-instance` / `open-url` | 视窗口是否显示 |

多实例时优先唤醒最近活跃的休眠实例；唤醒描述不可用时冷启动新实例。

### 21.5 两个落点

- **连接侧**（`crates/core` + `crates/native` / Web 驱动层）是唯一的生命周期控制器：空闲判定、`app/sleep`、租约、休眠 / 唤醒 / 快速恢复、线程释放、`onIdleExit`。
  平台封装只做两件事：上报可见性；把 OS 激活参数交给 `handleWake`。
- **注册侧**（`useTool`、`data-mcp-*`、`@mcp` 注释、状态库适配、原生 `ToolSpec`、静态清单）：注册表只在核心里，休眠不清空；
  休眠期间的注册变更**不触发唤醒**，只更新 `toolsHash`，回连时摘要不一致即完整同步；清单新增 `wake` 字段作为 App 未运行时的唤醒兜底；
  惰性 handler（Web `load: () => import()`、原生工厂闭包）让冷启动只初始化被调用的模块；调用进行中自动视为非空闲。框架适配层无需任何改动。

公开 API：`config.lifecycle`、`handleWake(args)`、`wake()`、`sleep()`、`hold()`、`connectNow()`、`onIdleExit`；连接状态新增 `dormant`、`waking`。

### 21.6 资源释放验收

- 休眠态：无打开的 socket；无运行中的定时器（核心 `poll_timeout()` 返回 `None`）。
- 原生运行时：休眠时停止运行时线程（或挂起到无定时器的 park），分发线程阻塞等待、不轮询，唤醒时重建。
- Web：休眠态无 WebSocket、无 `setTimeout` / `setInterval`；WASM 实例保留。
- 移动端：不申请前台服务、WakeLock、`beginBackgroundTask`。
- 核心测试覆盖：空闲计时的每个重置条件、租约、`app/sleep` 被拒后重试、快速恢复、休眠中注册变更导致完整同步。

排期见 M2（2.23–2.26）：阶段 A（协议与核心）→ 阶段 B（Web 驱动、绑定透传、平台唤醒入口）、注册侧、Host 配合。

## 22. 附录：相关方案对比

### 22.1 对比

| 方案 | 方向 | 覆盖面 | 与本项目的关系 |
|---|---|---|---|
| **WebMCP**（W3C 社区组草案，Chrome 149 起 Origin Trial） | 网页经 `document.modelContext.registerTool()` 与声明式表单注册工具，供**浏览器内置 AI** 调用 | 仅浏览器，仅当前标签页；无本机 MCP 客户端出口 | `@app-mcp/web/webmcp` 是其命令式 API 超集（polyfill / 桥接），`@app-mcp/dom` 识别声明式表单属性；工具双向镜像（6.7） |
| **MCP-B**（`@mcp-b/global`、`@mcp-b/webmcp-polyfill`、`@mcp-b/react-webmcp`、浏览器扩展） | WebMCP polyfill，并经扩展 / 桥接传输把页面工具交给 MCP 客户端 | 仅网页 | 与我们的网页端思路最接近；其 polyfill 在页面中时被当作"原生"对象桥接，经它注册的工具自动进入 Host；旧版返回值上的 `unregister()` 也兼容 |
| **webmcp-react / use-webmcp-tool** | React Hook，把组件内的工具注册到 `modelContext` | 仅 React + WebMCP | 装上 `installWebMcp` 后无需改动即可被 Host 调用；`@app-mcp/react` 的 `useTool` 同时镜像到浏览器原生 |
| **MCP-FE**（`@mcp-fe/*`） | 浏览器内 SharedWorker / ServiceWorker 记录事件、路由工具调用，经 Node 代理对外提供 MCP 端点；侧重查询前端状态与交互历史，支持 WebMCP | 仅网页 | 同属"网页作为 MCP 节点"；我们以 Host 聚合多 App、原生 SDK、按需唤醒、风险确认作为区别，状态查询由资源（`useResource`、状态库适配）覆盖 |
| **tauri-plugin-mcp** | Tauri 插件内置 MCP 服务器，提供截图、窗口管理、DOM 访问、输入模拟等调试 / 自动化工具 | 仅 Tauri | 属于界面层操作（对应 L3 / L4 与 `@app-mcp/inspect` 兜底），缺少业务语义；Tauri 应用可用 Rust crate 直接接入本项目（M3 `tauri-plugin-app-mcp`） |
| **Windows 原生 MCP**（On-device Agent Registry、agent connectors） | 系统级 MCP 服务器注册表，连接器默认在隔离的智能体会话中运行，要求静态声明工具集合 | 仅 Windows，公开预览 | Host 可登记为固定工具集合的网关连接器，ODR 中的其他连接器可作为上游导入；codegen 生成 MCPB 清单（8.6） |
| **App Actions on Windows / Agent Launchers** | App 以动作 JSON 注册原子动作（实体输入输出，URI / COM 激活）；Agent Launchers 在其上登记可对话智能体 | 仅 Windows，需包身份 | codegen 目标之一；调用路径第 2 级 |
| **App Intents**（苹果） | App 向 Shortcuts、Siri、Apple Intelligence 暴露意图 | 仅 Apple 平台 | codegen 目标之一；调用路径第 2 级；iOS 后台调用的主通道（21.4） |
| **AppFunctions**（安卓） | App 向系统智能体暴露函数 | 仅 Android 16 起 | codegen 目标之一；调用路径第 2 级 |
| **Playwright MCP / Chrome DevTools MCP** | 通过无障碍树或 DevTools 协议操作页面 | 仅网页，外部读取 | 对应本项目的 L3 思路，但缺少业务语义 |
| **MCP Apps / MCP-UI** | MCP 服务器返回 UI 给客户端渲染 | — | 方向相反，可互补 |
| **Computer use** | 截图 + 视觉 + 鼠标键盘 | 通用 | 对应本项目的 L4 兜底 |
| **D-Bus / Apple Events / COM** | 传统进程间调用 | 各平台 | 导入器来源；调用路径第 3 级 |
| **Deep Link / URI 协议 / x-callback-url** | 传统唤醒与传参 | 各平台 | 用于唤醒；`appmcp://` 在此基础上增加签名、schema 与结果回传 |

### 22.2 决定：做所有方案的超集，兼容并可导出到标准

上述方案各自只覆盖一段：WebMCP 系只管网页、只对浏览器内 AI；tauri-plugin-mcp 只管 Tauri 且停留在界面操作；
Windows ODR、App Actions、App Intents、AppFunctions 各限一个平台、各有一套声明格式。本项目不与它们竞争某一段，而是：

1. **兼容**：按标准写的代码直接可用——WebMCP 命令式 API 与声明式表单、MCP-B / webmcp-react / use-webmcp-tool 等库、已有 MCP 服务器（上游聚合）、ODR 中的连接器，不改一行即可进入 Host。
2. **导出到标准**：同一份工具定义镜像到浏览器原生 WebMCP，并由 codegen 生成 App Intents、AppFunctions、App Actions、ODR 的 MCPB 清单；Hub 可导出 MCP、OpenAI、Anthropic、Gemini 格式。
3. **超集能力**（任何单一方案都没有）：
   - **单一 Host 同时覆盖 web 与原生**：网页、Electron / Tauri、WPF、Qt、SwiftUI、Android、Flutter 的工具在同一个命名空间中聚合与路由；
   - **多语言**：Rust 核心 + 8 种语言 SDK，行为由一致性测试保证；
   - **按需唤醒**：静态清单 + 平台原生激活，App 未运行时工具仍可见，调用时唤醒、完成后休眠（第 10、21 节）；
   - **总览**：首次接触时附带 App 级使用说明，而不只是零散的工具描述（8.5）；
   - **多种声明方式**：HTML 属性、状态库、Hook / API、编译期注释、WebMCP 写法可以混用（6.6、6.7）；
   - **厂商可嵌入的 Hub SDK**：助手厂商无需 MCP、无需独立进程即可接入全部 App，并用自己的 UI 审批（第 20 节）。

理由：标准在快速演进（WebMCP 一年内多次改接口，Windows ODR 仍是预览），押注单一标准风险高；以自有协议为内核、把各标准作为输入与输出适配，
标准变化只影响对应的适配层（第 18 节），而开发者只需写一次。
