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

## 工具注解、输出 schema 与结构化结果

以下声明都可选，不写时与之前完全相同（语义见 spec/protocol.md 3.2）。

```ts
appMcp.tool('order.cancel', {
  description: '取消订单',
  input: z.object({ orderId: z.string() }),
  // 标准 MCP 工具注解：与 risk 同时给出时声明的字段逐个优先，缺少的按 risk 推导
  annotations: { destructiveHint: true, idempotentHint: true, openWorldHint: false },
  // 结果的 schema（MCP outputSchema）：JSON Schema、zod（只按输出形态转换，不校验）或带 toJSONSchema() 的对象
  outputSchema: z.object({ refunded: z.boolean() }),
  handler: async ({ orderId }) => {
    await requestCancel(orderId) // 页面弹出确认，用户确认后才真正取消
    return { data: { refunded: false }, status: 'pending', stateResource: 'order.status', summary: '已提交取消申请，等待用户在页面确认' }
  },
})
```

- **注解**原样转给 Agent（MCP `tools/list`），是否确认 / 放行由 Agent 决定；本库不据此拦截调用，高风险操作请在页面内自行确认。
- **outputSchema** 根类型不是 `object` 时由 Host 包装为 `{ result: … }`。Host 可按配置核对结果是否符合（默认只记日志）。
- **结构化结果**：handler 返回的对象含 `data` 键、且其余键都属于 `stateHints` / `status` / `stateResource` / `summary` / `annotations`
  并取值合法时，按结构化结果拆开；否则整个返回值作为 `data`。`status`：`done`（缺省）/ `pending`（已受理、尚未完成，附
  `stateResource`）/ `partial`（只完成一部分，用 `summary` 说明）/ `noop`（没有改动）。`annotations` 是结果内容的标注
  （`audience` / `priority` / `lastModified`）。封装层可用导出的 `isToolResultEnvelope(value)` 按同一规则判断。
- **无返回值**（`undefined` / `null`）且没有 `summary` 时，Host 对模型输出"已完成"，而不是 `null`。
- **幂等键**：handler 上下文 `ctx.idempotencyKey` 是 Agent 给出的幂等键（原样，跨重试不变；没有时缺省，spec/protocol.md 3.3），
  可作为业务层去重键或传给后端。Electron / Tauri 页面经桥接同样拿到。
- **资源的内容标注**：`appMcp.resource(name, { description, annotations: { audience: ['user'], priority: 0.5 }, read })`，
  Hub 放到 MCP `resources/list` 的资源注解上；缺省不声明。
- **调用调度**（spec/protocol.md 5.3，只在 SDK 内生效、不发给 Host）：`createAppMcp({ maxConcurrentCalls, maxQueuedCalls })`
  控制同时执行的调用数（缺省 1）与排队中的调用数（缺省 64，0 = 不限；队列满时新调用以 `RATE_LIMITED` 拒绝，`data` 为
  `{ scope: 'queue', limit }`）。工具可声明 `concurrency`（本工具同时执行的上限，缺省 / 0 = 不单独限制）与 `exclusive`
  （互斥组名，同组工具同一时刻至多一个在执行，如操作同一份文档的写工具）；不继承 scope，`update` 中给出 `undefined` 即清除。
- **标准意图**（spec/intents.md）：`appMcp.tool('compose.send', { description, input, implements: ['message.send@1'], handler })`
  声明工具实现了通用动词（每项 `<动词>@<主版本>`，最多 4 项），Agent 用内置工具 `apps.intents` 按动作找 App；格式不合法时注册失败，
  未知动词或缺少词表必填参数只记警告。不继承 scope，`update({ implements: undefined })` 清除。
  Electron / Tauri 页面经桥接同样声明（Electron 由主进程转给 `@app-mcp/node`）。
- **用户正在操作**（spec/protocol.md 5.3，只在 SDK 内生效、不发给 Host）：`appMcp.setBusy(true / false)` 声明用户此刻正在 App 内
  操作（何时算由 App 决定，如编辑框获得焦点、拖拽中），`isBusy()` 读取。期间写调用（生效注解不是 `readOnlyHint: true` 的工具）按
  `createAppMcp({ busyPolicy })` 处理：`'reject'`（缺省）以 `RATE_LIMITED` 拒绝（`data` 为 `{ scope: 'busy' }`，未执行，同一 `callId`
  稍后可重发）；`'queue'` 排队，`setBusy(false)` 后按到达顺序执行。只读调用与已开始的调用不受影响；`setBusyPolicy(policy)` 运行时修改
  （如由用户在设置中选择）。`beginBusy()` 开始一个作用域（可嵌套、按引用计数，句柄 `release()` 结束）：有效值 = 显式开关 OR 有未结束的
  作用域，`setBusy(false)` 不结束作用域、作用域结束也不清除显式开关，`isBusy()` 返回有效值。React 中用 `useBusy(active)`
  （`@app-mcp/react`，基于作用域）。Electron / Tauri 页面的有效值经桥接转给主进程 /
  Rust 侧：客户端的 busy 为各页面声明之或，页面刷新 / 关闭后失效；`busyPolicy` 在主进程（`@app-mcp/node`）/ Rust 侧
  （`NativeConfig::busy_policy`）配置，页面没有 `setBusyPolicy`。
- Host 还会对调用限流、限制参数 / 结果 / 资源大小，超出时 Agent 收到 `RATE_LIMITED` / `PAYLOAD_TOO_LARGE`
  （参数超限与被限流的调用不会转发到页面；结果超限时 handler 已执行，结果不返回）。上限见 crates/host/README.md。
- **需要用户操作**（登录过期、系统权限未授予、需切到前台、需在 App 内确认）时抛出
  `ToolCallError.userActionRequired('登录已过期，请重新登录后重试', { reason: 'login', uri: 'shop://login' })`：
  Agent 收到 `USER_ACTION_REQUIRED`，把说明转告用户；`reason` / `uri` 可省略。

## 事件（spec/protocol.md 3.5）

App 告诉 Agent "发生了什么"（订单已发货、下载完成）。先声明、再发出；Agent 用内置工具 `apps.events.subscribe` 订阅、
`apps.events` 取件（spec/hub-api.md 3.17）：

```ts
appMcp.declareEvent({
  name: 'order.shipped',
  description: '订单已发货；载荷含订单号',
  payloadSchema: { type: 'object', properties: { orderId: { type: 'string' } } },
})
const sent = appMcp.emitEvent('order.shipped', { orderId: 'o1' })   // 未连接时 false：丢弃，不缓存、不为此连接
appMcp.removeEvent('order.shipped')
```

- `declareEvent` 同名替换；核心加载前也可调用（加载时同步），已连接时随即同步给 Host。清单里的静态 `events`（`@app-mcp/build` 的
  `events` 选项）只用于 App 未运行时展示，运行时仍需声明。
- `emitEvent` 已连接时发送并返回 `true`；未连接（休眠、断线、重连中、核心尚未加载）返回 `false`，事件丢弃，不触发连接或唤醒，
  也不推迟空闲休眠。需要可靠送达的状态变化请用资源（`notifyChanged`）。
- 本地错误（抛出、不发送）：未声明或名称不合法 → `code` 为 `INVALID_NAME`；载荷不是 JSON 对象或序列化后超过 8 KiB
  （`MAX_EVENT_PAYLOAD_BYTES`）→ `INVALID_JSON`；`payloadSchema` 不是对象 → `INVALID_SCHEMA`。
- Electron / Tauri 页面经桥接（`event.declare` / `event.remove` / `event.emit`）由主进程 / Rust 侧发出；声明归本页，页面刷新 / 关闭后
  撤销（其他页面也声明了同名事件时保留）。页面按镜像的连接状态判断是否发送，与对方断线竞争时事件在对方丢弃。

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

## 界面级暴露与页面导航

工具可声明对界面的依赖（spec/protocol.md 3.4）：`surface: 'app'`（缺省）不依赖界面，后台可调、可唤醒；`surface: 'view'`
依赖界面，只在**真正可见且处于最上层**时对 Host 可见。`page` 声明工具所在页面（与清单 `pages[].name` 同名）：Host 调用不在
当前页面的工具时先请 App 导航到该页面，等工具注册后再派发。

```ts
const page = appMcp.scope('cart', { page: 'cart', surface: 'view', anchor: () => document.querySelector('#cart') })
page.tool('cart.checkout', { description: '结算', handler: checkout })   // 继承 page / surface / anchor

const dialog = createViewLayer('结算确认')          // 对话框 / 抽屉 / 模态框
dialog.open()                                       // 下层 view 工具暂停（tools/changed），层内工具启用
appMcp.scope('confirm', { layer: dialog }).tool('checkout.confirm', { description: '确认', surface: 'view', handler })
dialog.close()

appMcp.setNavigationHandler(async ({ page, params }) => router.push(pagePath(routes[page], params)))
```

- **可见性门控**（`view` 工具，`visibility: 'always'` 关闭）：页面可见（`document.visibilityState`）、处于最上层（不在已打开的
  `ViewLayer` 之下）、锚点（`anchor`，缺省继承 scope）已挂载且未被 `hidden` / `inert` / 打开的模态 `<dialog>` 遮挡、
  已渲染（`checkVisibility`）、在视口内。变化经 `IntersectionObserver`、属性变化与 `visibilitychange` 触发；不满足时工具对 Host
  表现为禁用（`enabled: false`），`enabled` 仍是 App 自己的意愿，两者同时满足才可见。界面以其他方式变化后可调用
  `refreshViewTools()`。
- **scope 的界面声明**（`appMcp.scope(name, { anchor, layer, page, surface, visibility })`）：其下工具未自行声明时继承，最近的
  scope 优先；层内工具不继承层外的 `page`（层只在打开时存在，不作为导航目标）。
- **导航回调**（`setNavigationHandler(handler, { whileLayerOpen?, settleMs? })`）：在启动时设置（握手时声明
  `capabilities.navigate`，连接后才设置的在下次连接生效）。回调切换界面后返回，SDK 再等界面稳定（两帧，上限 `settleMs`，默认
  500 ms）并重新评估门控后回复 Host。`ToolCallError.navigationDenied(msg)` 拒绝（`NAVIGATION_DENIED`）、
  `ToolCallError.navigationFailed(msg)` 或其他异常为失败（`NAVIGATION_FAILED`）；有打开的 `ViewLayer` 时缺省拒绝（用户正在与
  弹层交互，`whileLayerOpen: 'allow'` 改为照常调用）。
- **路由适配**：React Router 见 `@app-mcp/react` 的 `useRouterNavigation`；Vue Router 用子路径 `@app-mcp/web/vue-router`
  （`bindVueRouter(appMcp, router, { pages?, guard? })`，页面名缺省即路由 `name`，路由守卫拒绝 → `NAVIGATION_DENIED`）。
  框架无关的 `pagePath(pattern, params)`（填入 `:id` 等路由参数，其余作为查询串）与 `routePages(routes)`（带 `id` 的路由 →
  `{ 页面名: 路由模式 }`）供自写适配使用。
- Electron / Tauri 页面侧（桥接模式）同样支持门控与 `setNavigationHandler`（主进程 / Rust 侧需开启导航转发）；
  没有 AppMcp 实例时用 `attachBridgeNavigation(bridge, handler)`。
- **后台时**：标签页不可见时，需要前台的导航立即以 `USER_ACTION_REQUIRED`（`reason: "foreground"`）返回、不调用回调
  （`navigateInBackground` 缺省 `false`：浏览器标签页无法自行回到前台；置为 `true`（选项或 `setNavigateInBackground`）时交给回调，
  回调可抛出 `ToolCallError.userActionRequired(msg, { reason: 'foreground', uri })`，如发通知请用户点开）。后台也要能用的能力
  做成 `app` 工具，或给 `view` 工具声明 `backgroundTool`（同一 App 中一个 `app` 工具的名称，页面在后台时 Hub 改调它）；
  详见 spec/protocol.md 3.4「后台与前台」。桥接模式下 `navigateInBackground` 由主进程 / Rust 侧决定。

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
| `annotations` 中取值为布尔的 `readOnlyHint` / `destructiveHint` / `idempotentHint` / `openWorldHint` | 同时作为 `annotations` 原样转给 Agent |
| `execute` 返回 `{ content: [{ type: 'text', text }] }` | text 是 JSON → 解析为结果数据；否则 `{ text }`（有 `structuredContent` 时优先用它） |
| `execute` 返回 `{ ..., isError: true }` 或抛错 | `HANDLER_ERROR` |
| `execute` 返回其他值 | 原样作为结果数据（`undefined` → `null`） |

反向（`appMcp.tool()` → 原生）：`risk: 'read'` → `readOnlyHint`；`destructive` → `consequentialHint` + `destructiveHint`；
`payment` / `os-sensitive` → `consequentialHint`；`appMcp.tool()` 声明的 `annotations` 覆盖按 `risk` 推导的同名字段。结果以 `{ content: [{ type: 'text', text }] }` 返回，错误带 `isError: true`。

**同名冲突**：`appMcp.tool()` 优先。已有同名的 `appMcp.tool()` 工具时，标准侧 `registerTool` 以 `InvalidStateError` 拒绝；
标准侧先注册、`appMcp.tool()` 后注册时，标准侧的版本被移除（输出警告）。

**事件**：Host 调用经标准接口注册的工具时，在 `modelContext` 上派发 `toolactivated`，调用被取消 / 超时时派发
`toolcancel` 并 abort `execute` 收到的 `signal`，与浏览器内置 AI 调用时一致（声明式表单 `@app-mcp/dom` 依赖这些事件）。

### 安全说明

- **Host 不做确认**。Host 把 `risk` 与 MCP 注解如实交给 Agent，是否放行由 Agent 决定（spec/protocol.md 3.2）；
  `client.requestUserInteraction(cb)` 在 Host 调用时直接执行 `cb`，不弹出确认。需要用户确认的高风险操作请在页面内自行确认，
  并正确标注 `readOnlyHint` / `destructiveHint`（或 `consequentialHint`），未标注时按 `write` 处理。
- 浏览器内置 AI 调用时，确认流程由浏览器负责，本 SDK 不介入（原生提供 `requestUserInteraction` 时优先使用原生实现）。
- `exposedTo`（跨源暴露）只影响浏览器侧；Host 侧的访问控制由配对与 Host 配置决定，与页面来源无关。
- 工具返回的内容会原样交给模型，请同样遵循 WebMCP 的
  [安全建议](https://developer.chrome.com/docs/ai/webmcp/secure-tools)（如对用户生成内容设置 `untrustedContentHint`）。
