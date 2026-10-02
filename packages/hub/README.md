# @app-mcp/hub

> Part of [AppWire](https://github.com/stareing/appwire): embed the AppWire Hub in a Node.js agent (Electron assistant, LangChain.js, Vercel AI SDK, OpenAI / Anthropic SDK or your own loop) to list, call and approve the MCP tools of every app on the device, exported in MCP, OpenAI, Anthropic or Gemini tool-calling format.

Hub SDK 的 Node 绑定：把“连接本机所有 App”的能力嵌进 Node 端 Agent —— Electron 助手、LangChain.js、
Vercel AI SDK、OpenAI / Anthropic SDK 或自研循环。本机 App 通过 App 端 SDK（`@app-mcp/web`、`@app-mcp/node`、
Kotlin / Swift / C# …）连上嵌入的 Hub，你的 Agent 列工具、调用、读资源、收事件，并用自己的 UI 接管审批与配对。

契约见 `spec/hub-api.md`；Rust 库与概念说明见 `crates/hub/README.md`。原生部分为 `bindings/hub-node`（napi-rs）。

```bash
pnpm --filter @app-mcp/hub build:native   # cargo build -p app-mcp-hub-node，复制为 native/*.node
pnpm --filter @app-mcp/hub build          # tsup → dist/
```

## 启动

```ts
import { Hub } from '@app-mcp/hub'

const hub = await Hub.start({
  listen: '127.0.0.1:7717',            // HTTP 服务（/app 为 App 连接）；端口 0 = 随机；null = 不开
  approval: { requireAtOrAbove: 'destructive' },
  // manifestFiles / manifestDir / upstreams / 各超时（毫秒）…
})
console.log(hub.wsUrl)                   // App 端 SDK 的 hostUrl（ws://<listenAddr>/app）

hub.onEvent((e) => {                     // HubEvent 判别联合
  if (e.type === 'toolsChanged') refreshTools()
})
hub.setApprovalHandler(async (req) =>    // 可返回 boolean 或 Promise<boolean>
  showDialog(`${req.appName} 想要执行 ${req.title ?? req.tool}（风险：${req.risk}）`),
)
// 抛错、reject、超时、非 true 一律视为拒绝 → 调用以 USER_REJECTED 结束
```

`Hub.start` 缺省在运行期间保持 Node 进程存活（`keepAlive: false` 关闭），`await hub.shutdown()` 后释放。

## 三种接法

### A. 直接调用

```ts
for (const t of hub.tools({ maxRisk: 'write' })) console.log(t.name, t.risk, t.availability)

const out = await hub.callTool({ name: 'shop.cart.add', arguments: { sku: 'A1', qty: 1 }, session: 'conv-42' })
if ('error' in out.result && out.result.error) console.log(out.result.error.kind)   // USER_REJECTED / TIMEOUT …
else console.log(out.result.ok, out.stateHints)
if (out.overview) addToContext(out.overview.text)   // 该会话首次接触此 App
```

只有名称无法解析时 `callTool` 才 reject（`HubError`，`kind`/`code` 为错误类别）；工具层面的失败都在 `result.error`。
`CallOutcome` 另有 `status`（`'done'` / `'pending'` / `'partial'` / `'noop'`）、`stateResource`（`pending` 时可读后续状态的资源 URI）、
`summary` 与 `annotations`（App 对结果内容的 MCP 内容注解，原样）；`result.ok` 为原始返回值（无返回值为 `null`）。

接收进度：`hub.callTool(req, { onProgress: (p) => console.log(p.progress, p.total, p.message) })`——App 端 `ctx.progress(...)`
报告的进度经 Hub 合并（最小间隔 `progressIntervalMs`，缺省 250，丢弃不递增的值，`message` 截断到 200 字符）后在 Node 事件循环上
逐条回调，全部先于返回的 Promise 完成；调用结束后不再回调；回调抛错交给 `onListenerError`。不传 `onProgress` 时进度被丢弃。

### B. 自有 LLM：导出 + dispatch

`exportTools(format, filter?)` 与 `dispatch(format, toolCall, session?)` 支持 `mcp`、`openai-chat`、`openai-responses`、
`anthropic`、`gemini`。工具名编码为 `[a-zA-Z0-9_-]{1,64}`（`shop.cart.add` → `shop__cart__add`），dispatch 两种名字都接受。
下面的适配器是对这两个方法的薄封装（纯函数，不依赖任何 SDK 包）：

**OpenAI（Chat Completions）**

```ts
import OpenAI from 'openai'
import { toOpenAiTools, handleOpenAiToolCalls } from '@app-mcp/hub'

const msg = (await openai.chat.completions.create({ model, messages, tools: toOpenAiTools(hub) })).choices[0].message
messages.push(msg)
messages.push(...(await handleOpenAiToolCalls(hub, msg.tool_calls)))   // [{ role: 'tool', tool_call_id, content }]
```

Responses API：`toOpenAiResponsesTools` / `handleOpenAiResponsesCalls(hub, response.output)`。

**Anthropic**

```ts
import { toAnthropicTools, handleAnthropicToolUses } from '@app-mcp/hub'

const resp = await anthropic.messages.create({ model, max_tokens: 1024, messages, tools: toAnthropicTools(hub) })
messages.push({ role: 'assistant', content: resp.content })
const results = await handleAnthropicToolUses(hub, resp.content, { session: 'conv-42' })  // tool_result 块数组
if (results.length) messages.push({ role: 'user', content: results })
```

Gemini：`tools: [toGeminiTools(hub)]`，`handleGeminiFunctionCalls(hub, parts)`。

所有 `handle*` 都接受 `{ session, sequential }`：默认并发执行一批调用、结果按原顺序返回。

**Vercel AI SDK（v5）**

```ts
import { generateText, jsonSchema, stepCountIs } from 'ai'
import { toVercelAiTools } from '@app-mcp/hub'

const { text } = await generateText({
  model,
  prompt: '把牛奶加进购物车',
  tools: toVercelAiTools(hub, { includeBuiltin: true }, { jsonSchema, session: 'conv-42' }),
  stopWhen: stepCountIs(5),
})
```

返回 `{ [导出名]: { description, inputSchema, execute } }`。AI SDK 要求 `inputSchema` 为 `jsonSchema(...)` 或 zod，
所以请把 `ai` 包的 `jsonSchema` 作为第三个参数传入（不传时 `inputSchema` 是原始 JSON Schema，可用于其他框架，
如 LangChain.js 的 `tool(fn, { name, description, schema })`）。`execute` 返回工具结果文本，失败时抛 `HubToolCallError`
（AI SDK 会把它作为 tool-error 交给模型）；`abortSignal` 触发时取消调用。

### C. 对外开 MCP

```ts
const addr = await hub.serveHttp('127.0.0.1:0')      // 额外监听器（/app、/mcp、/healthz）：http://<addr>/mcp
// 或在 Hub.start 时设 mcpHttp: true，在 listen 上直接提供 /mcp
```

不经 `initialize`、每请求自带协议 `_meta` 的无会话 MCP 请求（spec/hub-api.md 3.6 / 3.7）另有配置：`taskIdleTtlMs`（其 Agent 任务的空闲回收，
默认 600000）、`statelessToolExposure`（默认 `'all'`）、`principalSelectTtlMs`（主体级 `apps.select` 的有效期，默认 60000）、
`statelessListTtlMs`（列表结果的 `ttlMs`，默认 5000）。`hub.status().tasks` 列出各调用方的 Agent 任务；MCP 出口发起的审批另带
`principal` 与 `clientName`（客户端自报，仅供显示，不得据此授权）。

## 资源保护与结果校验（spec/hub-api.md 3.11）

Hub 对 App 与上游工具的调用限流、限制数据大小，超出时调用以明确错误结束（不静默丢弃、不截断）：

```ts
const hub = await Hub.start({
  limits: { toolRatePerMinute: 60, toolRateBurst: 10 },   // 缺省字段取默认值；未知字段 → 启动失败
  outputValidation: 'reject',
})
```

| 配置 | 默认 | 超出 / 不符时 |
|---|---|---|
| `limits.toolRatePerMinute` / `toolRateBurst` | 120 / 30 | 每（App, 工具）令牌桶；`result.error.kind === 'RATE_LIMITED'`（-32016），`details`：`retryAfterMs`、`scope`（`'tool'` / `'app'`）、`perMinute`、`burst`、`appId`、`tool` |
| `limits.appRatePerMinute` / `appRateBurst` | 600 / 60 | 每 App（所有工具合计）令牌桶；同上 |
| `limits.maxArgumentsBytes` | 1 MiB | `'PAYLOAD_TOO_LARGE'`（-32017），`details`：`part`（`'arguments'` / `'result'` / `'resource'`）、`sizeBytes`、`limitBytes` |
| `limits.maxResultBytes` | 4 MiB | 同上；调用可能已在 App 内执行 |
| `limits.maxResourceBytes` | 4 MiB | 同上（`readResource` reject） |
| `outputValidation` | `'log'` | 结果与工具声明的 `outputSchema` 不符时：`'off'` 不校验 / `'log'` 只记日志、照常返回 / `'reject'` 以 `HANDLER_ERROR` 结束（`details.outputSchemaError`） |

- `*PerMinute` 为 0 表示该级不限，大小上限为 0 表示不限；`*PerMinute > 0` 而 `*Burst` 为 0 时 `Hub.start` 失败。
- 检查顺序：参数大小 → 两级限流 → schema 校验 / 审批 / 路由 / 唤醒（被限流的调用不会唤醒 App、不会触发审批）→ 转发 →
  结果大小 → `outputSchema` 核对。内置工具 `apps.*` 不受限；结果校验只针对 App 工具，无返回值不校验。
- 计数：`status()` 中 `AppStatus.rateLimited` / `tooLarge`（启动以来被拒绝的次数）、`AppStatus.tools`（`ToolDeclaration`：`risk`、
  声明的 `annotations`、实际生效的 `effective`、是否有 `outputSchema`），以及 `HubStatus.limits`（全部字段给出）与 `HubStatus.outputValidation`。

## 策略挂点（spec/hub-api.md 3.13）

`policy` 配置（运行中用 `hub.setPolicy(...)` 替换，`hub.policy()` / `status().policy` 查看规则与命中次数）：`hide` 使工具（或整个 App）从所有列表中消失、调用为 `TOOL_NOT_FOUND`；`deny` 使调用（`hooks: ['wake']` 时唤醒）以 `POLICY_DENIED`（-32018，`details.ruleId`）结束。规则不合法时 `Hub.start` 失败 / `setPolicy` 抛 `INVALID_INPUT`（旧规则继续生效）。

```ts
const hub = await Hub.start({ policy: { rules: [{ id: 'no-pay', action: 'deny', app: 'shop', tool: 'order.*' }] } })
hub.setPolicy({ rules: [{ id: 'no-destructive', action: 'hide', app: '*', annotations: { destructiveHint: true } }] })
```

**注解如实传递，不用于放行**：App 声明的标准 MCP 工具注解（`readOnlyHint` / `destructiveHint` / `idempotentHint` / `openWorldHint` / `title`）
原样出现在 `HubTool.annotations`、`ApprovalRequest.annotations` 与 `exportTools('mcp')` 中（缺少的字段按 `risk` 推导）。
`approval.requireAtOrAbove` 仍按 `risk` 决定是否询问；要按注解决定是否确认，在 `setApprovalHandler` 的回调里自行判断。

## 休眠与唤醒（spec/hub-api.md 3.5）

App 端以 `lifecycle: { mode: 'idle' }` 等方式运行时，空闲后与 Hub 完成 `app/sleep` 握手并断开，进入**休眠**：

- 工具仍列出，`availability` 为 `'dormant'`（`onlyAvailable: true` 不含它们）；`apps()` 中该 App 的 `connected` 为 false、
  `dormantInstances` 列出休眠实例；收到事件 `{ type: 'appDormant', appId, instanceId }`（不另发 `toolsChanged`）。
- 调用这些工具时 Hub 生成一次性令牌、发 `{ type: 'appWaking', appId, instanceId }`（`instanceId` 为 null = 冷启动），
  调用**唤醒实现**，等 App 回连后再派发（`wakeTimeoutMs` 内未回连 → `APP_NOT_RESPONDING`）。

默认唤醒实现按平台执行系统命令（URI scheme、AUMID、Apple Event、D-Bus、打开网页）。需要其他方式时（例如 Android 厂商发送
显式广播、或 Electron 助手里自己拉起进程）用 `setWaker` 替换：

```ts
hub.setWaker(async (req) => {
  // req: { appId, instanceId, descriptor: { kind, target, background }, token, activationArg: 'app-mcp-wake:<token>' }
  if (req.descriptor.kind !== 'uri') throw new HubError('LAUNCH_FAILED', `不支持 ${req.descriptor.kind}`)
  await launchMyApp(req.descriptor.target!, [req.activationArg])   // App 端把参数交给 handleWake()
})
// resolve = 已发出激活；抛错 / reject → LAUNCH_FAILED；抛 HubError('APP_NOT_INSTALLED', …) 等协议类别则按该类别结束调用
hub.setWaker(null)                                                 // 恢复配置 waker 决定的实现
```

相关配置（毫秒）：`leaseTtlMs`（调用后给实例的租约，默认 60000，0 关闭）、`wakeTimeoutMs`（默认 15000）、
`wakeTokenTtlMs`（默认 60000）、`dormantTtlMs`（休眠记录保留，默认 24 小时）、`dormantReplacedByNewInstance`（默认 true）、
`wakeFromLaunch`（App 未运行且清单无显式 `wake` 时由 `launch` 推导唤醒方式，默认 false）、
`waker`（`'system'` 默认 / `'none'` 不唤醒 / `{ exec: ['node', 'wake.mjs'] }`）。

## 页面、导航与 Agent 显式控制（spec/hub-api.md 3.14 / 3.15）

- `HubTool.surface`（`'app'` / `'view'`）与 `page`：App 工具的界面依赖与所在页面；内置与上游工具缺省。
- 调用不在当前页面的工具时 Hub 自动导航，等待上限 `navigateTimeoutMs`（默认 5000，独立于 `wakeTimeoutMs`）；超时 →
  `NAVIGATION_FAILED`。改调了 view 工具声明的后台替代时，`CallOutcome.routedTo` 为实际调用的工具全名。
- `callTool({ name, arguments, idempotencyKey: 'order-7' })`：Agent 的幂等键（1..=256 个字符）原样转交 App（handler 的
  `context.idempotencyKey`）；不合法时结果为 `INVALID_INPUT`。
- 内置工具 `apps.activate`（只唤醒不调用）/ `apps.release`（收回本会话租约）总是列出；有页面目录时另有 `apps.page` / `apps.navigate`。

## 渐进暴露（工具很多时）

`toolExposure: 'auto'`（默认）下，App 与上游工具总数超过 `toolExposureThreshold`（默认 40）时，`tools()` / `exportTools()`
只返回内置工具（`apps.list` / `apps.select` / `apps.overview` / `apps.tools` / `apps.activate` / `apps.release`，有页面目录时另有
`apps.page` / `apps.navigate`），以及该会话展开过、调用过或选定了实例的 App 的工具。
模型调用 `apps.tools({ appId })` 得到该 App 的工具（含 schema），之后这些工具出现在同一会话的导出里；未列出的工具按全名 / 导出名
仍可直接调用。会话由 `ToolFilter.session` 与 `dispatch(format, call, session)` 的会话对应：

```ts
const tools = toAnthropicTools(hub, { session: convId })      // 每轮重新导出
const results = await handleAnthropicToolUses(hub, content, { session: convId })
```

`toolExposure: 'all'` 恢复全部列出；`'progressive'` 始终渐进。给出 `ToolFilter.apps` 时列出这些 App 的全部工具。

## 自有 LLM 循环示例（Anthropic HTTP，无 SDK）

```ts
import { Hub, toAnthropicTools, handleAnthropicToolUses } from '@app-mcp/hub'

const hub = await Hub.start({ listen: '127.0.0.1:7717', approval: { requireAtOrAbove: 'destructive' } })
hub.setApprovalHandler(async (req) => confirm(`允许「${req.appName}」执行 ${req.tool}？`))

let tools = toAnthropicTools(hub)
hub.onEvent((e) => { if (e.type === 'toolsChanged') tools = toAnthropicTools(hub) })

async function chat(session: string, userText: string): Promise<string> {
  const messages: any[] = [{ role: 'user', content: userText }]
  for (let step = 0; step < 8; step++) {
    const resp = await fetch('https://api.anthropic.com/v1/messages', {
      method: 'POST',
      headers: {
        'content-type': 'application/json',
        'x-api-key': process.env.ANTHROPIC_API_KEY!,
        'anthropic-version': '2023-06-01',
      },
      body: JSON.stringify({ model: process.env.MODEL, max_tokens: 1024, messages, tools }),
    }).then((r) => r.json())
    messages.push({ role: 'assistant', content: resp.content })
    const results = await handleAnthropicToolUses(hub, resp.content, { session })
    if (results.length === 0) {
      return resp.content.filter((b: any) => b.type === 'text').map((b: any) => b.text).join('')
    }
    messages.push({ role: 'user', content: results })    // 成功为状态说明 / 摘要 / 返回值 JSON（无返回值为"已完成"）；失败带 is_error（如 USER_REJECTED: …）
  }
  return '（步数用尽）'
}

console.log(await chat('conv-1', '把牛奶加进购物车'))
hub.resetSession('conv-1')      // 开始新对话时重置（总览会再次附带）
await hub.shutdown()
```

## API 一览

| 方法 | 说明 |
|---|---|
| `Hub.start(config)` | 启动；`listenAddr` 为实际监听地址，`wsUrl`（`ws://<listenAddr>/app`）为 App 端点 |
| `shutdown()` | 关闭 App 连接与后台任务；之后调用抛 `HubError('SHUTDOWN')` |
| `apps()` / `tools(filter?)` / `resources()` / `overview(appId)` | 查询快照 |
| `status()` | 运行状态（`HubStatus`：监听、令牌策略、各 App 状态与最近错误、限流 / 超限计数与工具声明、SDK 诊断上报；与 `GET /status` 相同） |
| `callTool(req, { onProgress? }?)` / `cancelCall(callId)` | 调用（可接收进度）与取消（`req.timeout` 毫秒） |
| `readResource(uri)` / `subscribe(uri)` / `unsubscribe(uri)` | 资源（`app-mcp://<appId>/<name>`） |
| `selectInstance(appId, instanceId?)` / `resetSession(session?)` | 路由与会话 |
| `exportTools(format, filter?)` / `dispatch(format, call, session?)` | 格式导出与分派 |
| `onEvent(cb)` → 取消函数；`waitForEvent(pred, ms?)` | 事件 |
| `setApprovalHandler(fn)` / `setPairingHandler(fn)` | 审批 / 配对回调（可 async） |
| `setWaker(fn \| null)` | 自定义唤醒（休眠实例 / 冷启动）；`null` 恢复默认 |
| `serveHttp(addr, allowRemote?)` | MCP Streamable HTTP 出口 |

类型（`HubTool`、`CallRequest`、`CallOutcome`、`HubEvent`、`ApprovalRequest`、`WakeRequest` …）与 `crates/hub` 的 serde camelCase JSON 一致。

## 测试

```bash
pnpm --filter @app-mcp/hub build:native
pnpm --filter @app-mcp/node build:native   # 集成测试用 @app-mcp/node 作为 App 端
pnpm --filter @app-mcp/hub test
```

集成测试在同一进程中起嵌入式 Hub（随机端口）并用 `@app-mcp/node` 注册工具连上；缺少任一原生模块时跳过。
