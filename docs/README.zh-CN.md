<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/logo/appwire-logo-dark.png">
    <img src="assets/logo/appwire-logo-light.png" alt="AppWire" width="380">
  </picture>
</p>

# AppWire — 把任何 App 变成 AI Agent 可调用的 MCP 工具

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#许可)
[![MCP](https://img.shields.io/badge/Model%20Context%20Protocol-compatible-8A2BE2)](https://modelcontextprotocol.io)
[![GitHub stars](https://img.shields.io/github/stars/stareing/appwire?style=social)](https://github.com/stareing/appwire)

[English](../README.md) · **简体中文** · [繁體中文](README.zh-TW.md) · [日本語](README.ja.md) · [한국어](README.ko.md) · [Español](README.es.md) · [Português (Brasil)](README.pt-BR.md) · [Français](README.fr.md) · [Deutsch](README.de.md) · [Русский](README.ru.md) · [Italiano](README.it.md)

**AppWire 是一个开源的 [MCP（Model Context Protocol）](https://modelcontextprotocol.io) SDK 与本机 Hub，
把网页、桌面、手机 App 中真实的业务动作暴露为 AI Agent 可调用的工具**——Claude、ChatGPT、Gemini、
Claude Code 或你自己的大模型循环都能用。工具就声明在已经干活的代码旁边（React hook、HTML 属性、
函数注释、Kotlin / Swift 函数），任何 MCP 客户端都能调用。不截屏、不靠 computer use、不做浏览器自动化。

- **每个平台一个 SDK，共用一个 Rust 核心**：React、纯 HTML、Node、Electron、Tauri、Rust、C/C++、
  C#（WPF、WinUI）、Kotlin / Android、Swift（iOS、macOS）、Python（Qt、Tk）、Dart / Flutter。
- **一个 Hub 接入本机所有 App**：对外提供 MCP（stdio、Streamable HTTP），也能导出 OpenAI、Anthropic、
  Gemini 的工具调用（function calling）格式，或直接嵌入你自己的 Agent。
- **标准进，标准出**：读入并生成 WebMCP、Apple App Intents、Android AppFunctions、Windows App Actions；
  聚合已有的 MCP 服务器。
- **默认安全**：每个工具有风险等级，支付与破坏性操作需要人工确认。

> **万物皆工具。**
> App 即能力，界面即声明，调用即唤醒。

> 项目开发期间的工作名是 **app-mcp**；包名、crate 名与可执行程序名（`@app-mcp/*`、`app-mcp-*`、
> `app-mcp-host`）仍沿用它。

## 目录

[示例](#示例) · [安装](#安装) · [试用](#试用) · [平台与包](#平台与包) · [工作方式](#工作方式) ·
[对比](#与其他方案的对比) · [理念](#理念) · [常见问题](#常见问题) · [文档](#文档)

## 示例

**React**

```tsx
useTool('cart.checkout', {
  description: '结算当前购物车',
  risk: 'payment',
  input: z.object({ addressId: z.string() }),
  handler: ({ addressId }) => checkout(addressId),
})
```

**纯 HTML**

```html
<button data-mcp-tool="cart.clear" data-mcp-desc="清空购物车">清空</button>
```

**函数注释**（配合 `@app-mcp/build`）

```ts
/** 估算某城市的配送天数。 @mcp */
export function deliveryEstimate(city: string, express?: boolean) { … }
```

**自有大模型循环**（嵌入式 Hub，Node）

```ts
const hub = await Hub.start({})
const tools = hub.exportTools('anthropic')            // 交给模型；也可 'openai-chat'、'openai-responses'、'gemini'、'mcp'
const results = await handleAnthropicToolUses(hub, response.content)
```

## 安装

各包随首个版本发布；在此之前请按[试用](#试用)从源码构建。

```bash
# Web
npm install @app-mcp/web @app-mcp/react        # 另有：@app-mcp/dom、@app-mcp/store、@app-mcp/build
# Node / Electron
npm install @app-mcp/node @app-mcp/electron
# 把 Hub 嵌入 Node 端 Agent（LangChain.js、Vercel AI SDK、OpenAI / Anthropic SDK）
npm install @app-mcp/hub
# Rust / Tauri v2
cargo add app-mcp-native                       # Tauri：tauri-plugin-app-mcp + @app-mcp/tauri
# 本机 Hub（供 Claude Code、Claude Desktop、Cursor 等 MCP 客户端连接的 MCP 服务器）
cargo install app-mcp-host
```

C/C++、C#、Kotlin、Swift、Python、Dart 的原生 SDK 在 [`sdks/`](../sdks) 与 [`bindings/`](../bindings) 下，各自带构建说明。

## 试用

```bash
# 构建 Host 并以常驻服务运行（一个进程服务所有 MCP 客户端）
cargo build -p app-mcp-host
target/debug/app-mcp-host serve            # 或：app-mcp-host service install（登录自启）
target/debug/app-mcp-host status           # 一行摘要
target/debug/app-mcp-host doctor           # 连不上时：逐项检查，给出结论与修复建议

# 启动示例商城，然后在浏览器中打开
pnpm --filter @app-mcp/example-shop dev
```

Host 只用一个端口 `127.0.0.1:7717`：网页 App 连接 `/app`（WebSocket），MCP 客户端以 Streamable HTTP 连接 `http://127.0.0.1:7717/mcp`，`/healthz` 给出 Host 身份；原生 App 经当前用户专属的本地套接字（Unix 域套接字 / Windows 命名管道）连接，该套接字同样提供 MCP，供支持本地套接字的 Agent 使用。单实例锁保证每个用户只有一个 Host，实际监听位置记录在 `~/.app-mcp/run/endpoints.json`。
仓库的 `.mcp.json` 已让 Claude Code 连接该端点：重启会话、打开示例页面，就可以让 Claude 操作商城。其他 MCP 客户端同理：

```json
{ "mcpServers": { "appwire": { "type": "http", "url": "http://127.0.0.1:7717/mcp" } } }
```

连不上时运行 `app-mcp-host doctor`：检查 Host、单实例锁、本地套接字权限、端口被哪个进程占用、Windows 排除端口段、令牌模式、`adb reverse`，以及各 App 的状态与最近错误；SDK 的连接状态带机器可读的错误码（`spec/protocol.md` 第 10 节）。
配置文件、访问令牌与其他 MCP 客户端见 [`crates/host/README.md`](../crates/host/README.md)。

## 平台与包

| 平台 | 包 | 说明 |
|---|---|---|
| Web | `@app-mcp/web`、`@app-mcp/react`、`@app-mcp/dom`、`@app-mcp/store`、`@app-mcp/build` | WASM 核心；WebMCP 兼容；HTML 属性；Zustand / Redux / Pinia；编译期 `@mcp` |
| Node / Electron | `@app-mcp/node`、`@app-mcp/electron` | 主进程 + 渲染进程桥接 |
| Rust（Tauri、egui 等） | `crates/native` | 直接依赖 |
| Tauri v2 | `crates/tauri-plugin`、`@app-mcp/tauri` | 插件：Rust 工具 + WebView 页面经 Tauri IPC 登记（`@app-mcp/web` 用法不变） |
| C / C++ | `bindings/c`、`sdks/cpp` | 稳定 C ABI（`app_mcp.h`） |
| C#（WPF、WinUI） | `sdks/dotnet` | P/Invoke；单实例与协议激活辅助 |
| Kotlin / Android | `sdks/kotlin` | 协程；`WakeReceiver` + 加急 WorkManager |
| Swift（iOS、macOS） | `sdks/swift` | async/await；SwiftUI 生命周期修饰器 |
| Python | `sdks/python` | 同步或 asyncio；Qt / Tk 调度；D-Bus 唤醒 |
| Dart / Flutter | `sdks/dart` | dart:ffi；`AppLifecycleListener` 集成 |
| 鸿蒙 HarmonyOS NEXT（ArkTS） | `sdks/harmony`（`@app-mcp/harmony`）、`bindings/harmony` | Node-API 原生模块（与 `@app-mcp/node` 同一份绑定源码）；应用前后台与 `Want` 唤醒 |
| 系统原生意图 | `crates/codegen` | 生成 App Intents、AppFunctions、Windows App Actions、鸿蒙意图框架（InsightIntent）与类型化接口 |
| Agent / 厂商 | `crates/hub`、`@app-mcp/hub`、`bindings/hub-c`、`bindings/hub-uniffi` | 可嵌入的 Hub：Rust、Node、C/C#、Kotlin、Swift、Python |

## 工作方式

```mermaid
flowchart TD
  clients["MCP 客户端 · 自有大模型循环 · 厂商 Agent"]
  hub["AppWire Hub<br/>路由 · 总览 · 审批<br/>生命周期：休眠 / 唤醒 / 租约"]
  clients -- "MCP（stdio · Streamable HTTP）<br/>工具格式导出 + dispatch · 嵌入式 API" --> hub
  hub -- "WebSocket（本机）" --> web["Web SDK<br/>（WASM 核心）"]
  hub -- "WebSocket（本机）" --> desktop["桌面 SDK<br/>Rust · C/C++ · C# · Python"]
  hub -- "WebSocket（本机）" --> mobile["移动端 SDK<br/>Kotlin · Swift · Dart/Flutter"]
  hub -- "WebSocket（本机）" --> node["Node / Electron"]
  hub -- "子进程" --> upstream["已有的 MCP 服务器"]
  subgraph core["所有语言共用一个 Rust sans-IO 核心"]
    web
    desktop
    mobile
    node
  end
```

- **App 端 SDK** 注册工具与资源；协议由同一个 Rust 核心（`crates/core`）实现，各语言行为一致。
- **Hub**（`crates/hub`）聚合 App 与上游 MCP 服务器，把调用路由到正确的实例，首次接触时附带简短的 App 总览，
  执行审批，并唤醒休眠的 App。`app-mcp-host` 是它的命令行入口。
- **静态清单**（`app-mcp.json`）让 Hub 在 App 未运行时也能列出其工具并唤醒它。

## 与其他方案的对比

| 方案 | 模型看到的 | App 未运行时可用 | 平台 | 审批 |
|---|---|---|---|---|
| Computer use / 截屏类 Agent | 截图、像素 | 否 | 桌面 | 无内置 |
| 浏览器自动化（如 Playwright MCP） | DOM / 无障碍树 | 否 | 网页 | 无内置 |
| 每个 App 手写一个 MCP 服务器 | 工具，但与 App 代码分开维护 | 视实现而定 | 每个服务器一个 | 各自实现 |
| WebMCP | 页面声明的工具 | 否 | 仅浏览器 | 浏览器提示 |
| App Intents / AppFunctions / App Actions | 系统意图 | 是 | 各自一个操作系统 | 系统级 |
| **AppWire** | **App 自己代码里声明的工具** | **是（清单 + 唤醒）** | **网页、桌面、手机** | **按工具的风险等级** |

AppWire 不取代这些标准：它能读入并生成 WebMCP、App Intents、AppFunctions、Windows App Actions，
也能把已有的 MCP 服务器聚合到同一个 Hub 之后。

## 理念

Unix 说"一切皆文件"：设备、管道、进程都用 `open / read / write` 同一套接口。
"万物皆插件"说：功能以插件的形式装进宿主。

AppWire 说"**万物皆工具**"：按钮、表单、菜单命令、状态库动作、系统能力、已有的 MCP 服务器，
都以同一种形式表达——名称、参数格式、风险等级、处理函数。模型只需要三个动词：**列出、调用、读取**。

插件是把代码装进宿主；工具则相反——代码留在 App 里，App 声明自己能做什么，由模型来编排。

### 八条原则

1. **在动作所在处声明**：能力就写在它本来所在的地方——React hook、HTML 属性、函数注释、状态库、
   原生 `ToolSpec`。不另外维护一份描述；代码变了，工具跟着变。
2. **声明，而非解析**：不截图、不爬 DOM、不猜哪个按钮能点。App 自己说能做什么，模型拿到的是意图而不是像素。
   界面解析（`@app-mcp/inspect`）只作为默认关闭的兜底。
3. **工具随界面生灭**：打开页签，工具出现；关掉，工具消失；购物车为空，就没有"结算"。
   模型看到的永远是此刻真正能做的事。
4. **不用即睡，用时即醒**：空闲就断开连接、释放线程，不强占进程常驻；需要时由 Hub 用各平台原生方式唤醒，
   一次往返即可恢复。连接是手段，不是负担。
5. **一个枢纽，万端接入**：一个 Hub 同时服务网页、桌面、手机上的 App，对外讲 MCP、OpenAI、Anthropic、
   Gemini 的工具格式，也能直接嵌进厂商自己的 Agent。一次接入，处处可用。
6. **兼容即超集**：WebMCP、App Intents、AppFunctions、Windows App Actions 都能读入，也都能生成。
   不与标准竞争，只负责把它们连起来。
7. **描述不等于授权**：总览告诉模型这个 App 是做什么的，但不赋予任何权限。能否执行由风险等级与审批决定，
   人始终握有确认权。
8. **从源头解决**：问题出在哪一层，就改哪一层；不用转发进程、包装脚本、补丁替换或兜底转换去掩盖它。

## 常见问题

**怎么把 React 应用变成 MCP 服务器？**
加上 `@app-mcp/react`，用 `useTool` 包住要暴露的动作，运行 `app-mcp-host`。页面会连到本机 Hub，
组件挂载期间，连接 Hub 的所有 MCP 客户端都能看到这些工具。纯页面可以改用 `@app-mcp/dom` 的 `data-mcp-*` 属性。

**怎么让 Claude（或 ChatGPT、Gemini、Cursor）操作桌面或手机 App？**
用对应平台的 SDK（Electron、Tauri、C#、Kotlin、Swift、Python、Flutter 等）注册工具，让 MCP 客户端连接
`http://127.0.0.1:7717/mcp`。Android App 在开发阶段经 `adb reverse` 连到 Hub。

**每个 App 都要单独写一个 MCP 服务器吗？**
不用。App 都登记到同一个本机 Hub，Hub 就是所有客户端面对的唯一 MCP 服务器；已有的 MCP 服务器也可以作为上游挂在它后面。

**不用 MCP，能在自己的 Agent 里用吗？**
可以。嵌入 Hub（Rust、Node、C/C#、Kotlin、Swift、Python），以 OpenAI、Anthropic 或 Gemini 格式导出工具，
再把模型的工具调用经 Hub 分发回去。见 [`spec/hub-api.md`](../spec/hub-api.md)。

**和 computer use、浏览器自动化有什么不同？**
那些方案让模型读屏幕、猜该点哪里。AppWire 由 App 用带类型的输入格式声明自己的动作，调用精确、快速，
窗口被遮挡时也能用——App 没在运行时也能按需唤醒。

**让模型调用 App 的动作安全吗？**
每个工具都有风险等级（`read`、`write`、`destructive`、`payment`、`os-sensitive`）；有风险的调用需要在 Hub
中由人确认，App 总览本身不授予任何权限。

**支持 WebMCP 吗？**
支持。`@app-mcp/web/webmcp` 以 polyfill 形式实现 WebMCP 的 `modelContext` API 并做桥接，
按标准写的页面同样经 Hub 暴露。

## 文档

| 文档 | 内容 |
|---|---|
| [`spec/protocol.md`](../spec/protocol.md) | SDK ↔ Hub 协议（权威） |
| [`spec/manifest.md`](../spec/manifest.md) | 静态清单 `app-mcp.json` |
| [`spec/lifecycle.md`](../spec/lifecycle.md) | App 生命周期：休眠、唤醒、租约、快速恢复 |
| [`spec/hub-api.md`](../spec/hub-api.md) | 可嵌入 Hub 的 API 与各语言绑定 |
| [`crates/host/README.md`](../crates/host/README.md) | Host 配置、访问令牌、MCP 客户端 |
| [`llms.txt`](../llms.txt) | 面向大模型与 AI 搜索的项目摘要 |
| [`app-mcp-plan.md`](../app-mcp-plan.md) | 完整设计与路线图 |
| [`TASKS.md`](../TASKS.md) | 当前进度 |

## 状态

原型阶段（M1–M2）。协议、核心、Hub 与全部语言 SDK 已实现，并在 Linux 与 Windows 上通过测试，
Android 已真机验证；Apple 平台目前只在 Linux 上验证。API 仍可能变化。欢迎提 Issue 与 PR。

## 许可

可任选 [Apache License 2.0](../LICENSE-APACHE) 或 [MIT](../LICENSE-MIT)。
