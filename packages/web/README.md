# @app-mcp/web

> Part of [AppWire](https://github.com/stareing/appwire): the browser SDK that exposes your web app's actions as MCP (Model Context Protocol) tools for AI agents such as Claude, ChatGPT and Gemini, with a WebMCP polyfill and bridge. Works with Claude Code, Claude Desktop and any MCP client through the local AppWire Host.

浏览器 SDK：把网页中的业务动作以 MCP 工具暴露给模型（经本地 app-mcp Host 供 Claude Desktop / Claude Code 调用）。

```ts
import { createAppMcp } from '@app-mcp/web'

export const appMcp = createAppMcp({ appId: 'shop', appName: '示例商城' })
appMcp.tool('cart.clear', { description: '清空购物车', risk: 'destructive', handler: () => cart.clear() })
```

## 连接地址与 Host 身份

- 默认连接 `ws://127.0.0.1:7717/app`（Host 的 HTTP 端口：`/app` 是 App 连接，同一端口的 `/mcp` 给 MCP 客户端）。
- 未指定 `hostUrl` 时按 Host 的备选顺序依次尝试 `7717 → 7737 → 7757`（Host 的默认端口被其他程序占用时会改用备选端口）：
  连不上、或握手结果表明对端不是 app-mcp 时换下一个；连上后固定在该端口。显式指定 `hostUrl` 时只连它。
- 握手结果带 Host 身份（`service: "app-mcp"`、`user`、`pid`，spec/protocol.md 1.6）。对端不是 app-mcp（候选端口都不是）时
  进入 `{ status: 'host-mismatch', reason }`：不再定时重试，`wake()` / `connectNow()` 时再试一次。浏览器不知道操作系统用户，
  网页不核对 `user`；多用户机器上请显式指定 `hostUrl`。
- 旧地址 `ws://127.0.0.1:7717`（根路径）在兼容期内仍可用（Host 记录提示），请改为 `/app`。

## 惰性 handler

只声明元数据，handler 在首次调用时加载（与 `handler` 二选一）。加载结果缓存；加载失败时本次调用返回
`HANDLER_ERROR`，下次调用重试；`setHandler` 仍可直接替换 handler。

```ts
appMcp.tool('cart.checkout', { description: '结算', risk: 'payment', load: () => import('./checkout') })
```

## 生命周期（休眠与唤醒）

默认 `persistent`：启动即连接、一直在线。设置 `lifecycle` 后，空闲时与 Host 完成 `app/sleep` 握手并断开
（`dormant`：无 WebSocket、无定时器，WASM 实例与工具注册保留），需要时再回连（`waking`）。完整规范见 `spec/lifecycle.md`。

```ts
const appMcp = createAppMcp({
  appId: 'shop',
  appName: '示例商城',
  lifecycle: { mode: 'idle', idleTimeoutMs: 60_000, hiddenIdleTimeoutMs: 15_000 },
})
```

| 模式 | 行为 |
|---|---|
| `persistent`（默认） | 不自动休眠 |
| `idle` | 启动时连接；空闲 `idleTimeoutMs`（标签页隐藏 / 冻结时 `hiddenIdleTimeoutMs`）后休眠 |
| `on-demand` | 启动时不连接；被唤醒或调用 `connectNow()` 时连接，任务完成后经过 `graceMs` 休眠 |

- **空闲**：没有进行中的调用 / 资源读取、没有 Host 租约与资源订阅、没有持有。调用进行中自动视为非空闲。
- **唤醒**：页面重新可见（`idle` / `on-demand`）；地址带 `#app-mcp-wake=<token>`（Host 打开 / 聚焦页面时附带，
  SDK 读取后用 `history.replaceState` 从地址栏移除，其他 hash 保留）；`appMcp.wake()`。
- **bfcache**：`pagehide`（`persisted`）时立即请求休眠（所有模式），`pageshow` 恢复时回连；
  `idle` / `on-demand` 模式下 Page Lifecycle 的 `freeze` / `resume` 同理。
- **快速恢复**：回连时携带上次的恢复令牌与工具摘要，Host 确认未变化时跳过 `tools/sync`；
  休眠期间注册 / 注销工具不会唤醒，回连时摘要不一致即完整同步。
- **持有**：`const h = appMcp.hold()` 阻止休眠，直到 `h.release()`；handler 内发起后台长任务时用
  `context.hold?.()` 在调用结束后继续保持。React 中用 `useHold(active?)`（`@app-mcp/react`）。
- 其他：`appMcp.sleep()` 主动休眠（不看空闲条件）；`connectNow()` 在 `on-demand` 模式下主动连接。
  Electron 桥接模式下这些方法为空操作（生命周期由主进程的 `@app-mcp/node` 负责），状态照常透传。

## 运行环境

- **复制标签页**：instanceId 存在 `sessionStorage`，复制标签页会复制它。SDK 启动时按 appId 用
  `BroadcastChannel` 探测（与 WASM 加载并行），发现其他标签页占用同一 ID 时重新生成；没有 `BroadcastChannel` 时不检测。
- **Electron 渲染进程**：preload 用 `@app-mcp/electron/preload` 暴露了桥接对象（`window.appMcpBridge`，
  或页面上有 `getAppMcpBridge()`）时，`createAppMcp` 经 IPC 把工具登记到主进程，不加载 WASM、不连接 Host。

## 多个标签页共用一条连接

默认（`sharedConnection: true`）同一来源的所有标签页经 SharedWorker 共用**一条**到 Host 的 WebSocket
（协议见 `spec/protocol.md` 第 9 节「多路复用」）。每个标签页仍是独立实例：各自的 instanceId、各自注册的工具，
标签页关闭时它的工具随之消失；调用路由、焦点、`apps.select` 与之前完全相同。

| 环境 | 连接持有方 |
|---|---|
| 有 SharedWorker（桌面 Chrome / Edge / Firefox / Safari 16+，Chrome Android 148+） | SharedWorker（`mux-worker.js`） |
| 没有 SharedWorker，但有 Web Locks 与 BroadcastChannel | 用锁选出的主标签页；它进入 bfcache / 冻结 / 关闭时让位，其他标签页自动重连到新的主标签页 |
| 都没有 / `sharedConnection: false` | 每个标签页直接连接 |
| Host 不支持多路复用（旧 Host）、SharedWorker 脚本无法加载 | 自动改为每个标签页直接连接 |

- 打包：源码中以 `new SharedWorker(new URL('./mux-worker.ts', import.meta.url), { type: 'module', name: 'app-mcp' })` 创建，
  Vite / webpack 5 会单独输出 worker 文件；发布包中为 `dist/mux-worker.js`。页面 CSP 需允许 `worker-src 'self'`（否则自动改为直接连接）。
- 生命周期不变：空闲 / 隐藏休眠只关闭本标签页的通道；bfcache 前先 `app/sleep`，再请 worker 暂存发给本页的消息
  （向缓存中的页面投递消息会使其被逐出），恢复时取回；`#app-mcp-wake=` 唤醒照常。所有标签页都休眠时没有 WebSocket。

## 连接被浏览器拦截（`blocked` 状态）

浏览器拦截时 WebSocket 只报告一个无原因的错误，与"Host 没有运行"看起来一样。SDK 收集旁证判断原因，
确认是拦截时进入 `{ status: 'blocked', cause, message }`（`message` 为可直接展示的中文说明），**不再按退避定时重试**：

| `cause` | 含义 | 恢复 |
|---|---|---|
| `local-network-access` | Chrome 本地网络访问（LNA，Chrome 142 起 fetch、147 起 WebSocket）：公网 / 局域网页面连接本机需用户允许「本机上的应用」（权限 `loopback-network`，旧名 `local-network-access`），当前为拒绝 | 授权变为允许时（`navigator.permissions` 的 `change` 事件）立即重连；另每 60 秒低频探测一次 |
| `insecure-context` | 非 HTTPS 的公网页面：Chrome 禁止其访问本机，也不会询问 | 改用 HTTPS 部署，或在 localhost 打开 |
| `csp` | 页面（或 SharedWorker 脚本响应）的 CSP `connect-src` 不允许 Host 地址（`securitypolicyviolation` 事件） | 在 `connect-src` 中加入 `ws://127.0.0.1:7717`（Host 改用备选端口时还需 7737、7757）后刷新页面 |

- `appMcp.wake()` / `connectNow()` 在 `blocked` 时立即重试一次（LNA 下可重新弹出授权提示）。
- 授权为「询问」（`prompt`）时不判定为拦截：授权提示只能由页面发起，因此经 SharedWorker 的连接失败后，
  该次连接改由页面直接建立（触发提示）；仍失败则按普通断开退避重连（实测 Chrome 153 中该状态下 WebSocket 可能直接连通，
  失败与 Host 未运行无法区分）。
- Host 无需设置任何响应头：Private Network Access 的 CORS 预检（`Access-Control-Allow-Private-Network`）已被 LNA 的
  权限提示取代，且 WebSocket 从不预检。
- 页面在 localhost / 127.0.0.1 上时不受 LNA 限制。

```tsx
appMcp.onStateChange((s) => {
  if (s.status === 'blocked') showBanner(s.message)
})
```

## 与 WebMCP 标准的关系

[WebMCP](https://webmachinelearning.github.io/webmcp/) 是 W3C Web Machine Learning 社区组起草的标准：页面通过
`document.modelContext.registerTool()` 注册工具，供**浏览器内置的 AI**（Chrome 149 起以 Origin Trial 提供）调用。
app-mcp 解决的是另一段链路：把页面工具交给**本机的 MCP 客户端**（Claude Desktop、Claude Code 等）。

`@app-mcp/web/webmcp` 让两者打通，本 SDK 因此成为 WebMCP 命令式 API 的超集：

```ts
import { createAppMcp } from '@app-mcp/web'
import { installWebMcp } from '@app-mcp/web/webmcp'

const appMcp = createAppMcp({ appId: 'todo', appName: '待办' })
const uninstall = installWebMcp(appMcp) // 返回卸载函数；uninstall.mode 为 'polyfill' | 'bridge' | 'native-readonly'
```

安装后：

- **按标准写的代码不用改**：`document.modelContext.registerTool({ name, description, inputSchema, execute, annotations })`
  注册的工具同时进入 app-mcp，可被 Host 调用。
- **反过来**：用 `appMcp.tool()`（以及 `@app-mcp/react` 的 `useTool`）注册的工具，也注册到浏览器原生的
  `modelContext`，浏览器内置 AI 同样可以调用（`mirrorOwnTools`，默认开启）。

### 两种模式

| 场景 | 行为 |
|---|---|
| 浏览器**没有**原生 WebMCP（polyfill） | 在 `document.modelContext` 与 `navigator.modelContext` 上安装同一个标准兼容对象（`EventTarget`），提供 `registerTool` / `getTools` / `executeTool` 与 `toolchange` / `toolactivated` / `toolcancel` 事件 |
| 浏览器**有**原生 WebMCP（桥接） | 不替换原生对象，只在其实例上覆盖 `registerTool`，并补上 `unregisterTool` / `provideContext` / `clearContext`：页面注册的工具**同时**注册到原生（浏览器内置 AI 可用）与 app-mcp（Host 可用），注销时两边同步 |
| 原生对象的方法无法覆盖（`native-readonly`） | 页面经原生接口注册的工具只在浏览器内可用；`appMcp.tool()` 的工具仍镜像到原生 |

原生只存在于 `navigator.modelContext`（早期 Chrome）或只存在于 `document.modelContext`（当前标准）时，会给另一个入口补上指向同一对象的别名。

### 兼容的写法

标准在演进中移除过一些接口，但不少第三方库仍在使用。这里全部支持：

| 写法 | 状态 | 支持 |
|---|---|---|
| `document.modelContext.registerTool(tool, { signal })`，`signal` abort 即注销 | 当前标准 | ✓ |
| `registerTool()` 返回 Promise；重名 / 非法名 / 空描述 → `InvalidStateError` | 当前标准 | ✓ |
| `execute(input, { signal })` | 当前标准 | ✓ |
| `navigator.modelContext` | 2026-05 移到 `document` | ✓ 两个入口是同一对象 |
| `unregisterTool(name)` | 2026-03 移除 | ✓ |
| `provideContext({ tools })` / `clearContext()` | 2026-03 移除 | ✓ 只替换 / 清除**经 modelContext 注册的**工具，不影响 `appMcp.tool()` 的工具 |
| `execute(input, client)` 与 `client.requestUserInteraction(cb)` | 2026-06 移除 | ✓ 直接执行 `cb` |
| `registerTool()` 返回值上的 `unregister()`（旧版 MCP-B polyfill） | 非标准 | ✓ |

因此 `@mcp-b/global`、`@mcp-b/react-webmcp`、`use-webmcp-tool`、`webmcp-react` 等库：只要在它们注册工具之前调用
`installWebMcp(appMcp)`，工具就会自动出现在 Host 中，无需改动库或业务代码。如果页面已经装了其他 polyfill
（例如 `@mcp-b/global`），它会被当作"原生"对象桥接。

### 映射规则

| WebMCP | app-mcp |
|---|---|
| `name`（1–128 个 `[A-Za-z0-9_.-]`） | 工具名（app-mcp 限 64 个字符；更长的名字只在浏览器侧可用，并输出警告） |
| `title` / `description` / `inputSchema` | `title` / `description` / `input`（缺省为空对象 schema） |
| `annotations.readOnlyHint` | `risk: 'read'` |
| `annotations.destructiveHint` 或 `consequentialHint` | `risk: 'destructive'` |
| 其他 | `risk: 'write'` |
| `execute` 返回 `{ content: [{ type: 'text', text }] }` | text 是 JSON → 解析为结果数据；否则 `{ text }`（有 `structuredContent` 时优先用它） |
| `execute` 返回 `{ ..., isError: true }` 或抛错 | `HANDLER_ERROR` |
| `execute` 返回其他值 | 原样作为结果数据（`undefined` → `null`） |

反向（`appMcp.tool()` → 原生）：`risk: 'read'` → `readOnlyHint`；`destructive` → `consequentialHint` + `destructiveHint`；
`payment` / `os-sensitive` → `consequentialHint`。结果以 `{ content: [{ type: 'text', text }] }` 返回，错误带 `isError: true`。

**同名冲突**：`appMcp.tool()` 优先。已有同名的 `appMcp.tool()` 工具时，标准侧 `registerTool` 以 `InvalidStateError` 拒绝；
标准侧先注册、`appMcp.tool()` 后注册时，标准侧的版本被移除（输出警告）。

**事件**：Host 调用经标准接口注册的工具时，在 `modelContext` 上派发 `toolactivated`，调用被取消 / 超时时派发
`toolcancel` 并 abort `execute` 收到的 `signal`，与浏览器内置 AI 调用时一致（声明式表单 `@app-mcp/dom` 依赖这些事件）。

### 安全说明

- **风险确认由 Host 执行**。Host 根据 `risk`（即上表由 annotations 推导出的值）在调用前向用户确认；
  因此 `client.requestUserInteraction(cb)` 在 Host 调用时直接执行 `cb`，不再额外弹出确认。需要确认的工具请正确标注
  `readOnlyHint` / `consequentialHint`（或 `destructiveHint`），否则按 `write` 处理。
- 浏览器内置 AI 调用时，确认流程由浏览器负责，本 SDK 不介入（原生提供 `requestUserInteraction` 时优先使用原生实现）。
- `exposedTo`（跨源暴露）只影响浏览器侧；Host 侧的访问控制由配对与 Host 配置决定，与页面来源无关。
- 工具返回的内容会原样交给模型，请同样遵循 WebMCP 的
  [安全建议](https://developer.chrome.com/docs/ai/webmcp/secure-tools)（如对用户生成内容设置 `untrustedContentHint`）。
