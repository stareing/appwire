# @app-mcp/hub

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
  wsAddr: '127.0.0.1:7717',            // App 连接服务；端口 0 = 随机；null = 不开
  approval: { requireAtOrAbove: 'destructive' },
  // manifestFiles / manifestDir / upstreams / 各超时（毫秒）…
})
console.log(hub.wsUrl)                   // App 端 SDK 的 hostUrl

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
const addr = await hub.serveHttp('127.0.0.1:7718')   // Streamable HTTP：http://<addr>/mcp
```

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
hub.setWaker(null)                                                 // 恢复默认实现
```

相关配置（毫秒）：`leaseTtlMs`（调用后给实例的租约，默认 60000，0 关闭）、`wakeTimeoutMs`（默认 15000）、
`wakeTokenTtlMs`（默认 60000）、`dormantTtlMs`（休眠记录保留，默认 24 小时）、`dormantReplacedByNewInstance`（默认 true）、
`wakeFromLaunch`（App 未运行且清单无显式 `wake` 时由 `launch` 推导唤醒方式，默认 false）。

## 自有 LLM 循环示例（Anthropic HTTP，无 SDK）

```ts
import { Hub, toAnthropicTools, handleAnthropicToolUses } from '@app-mcp/hub'

const hub = await Hub.start({ wsAddr: '127.0.0.1:7717', approval: { requireAtOrAbove: 'destructive' } })
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
    messages.push({ role: 'user', content: results })    // 成功为 JSON 文本；失败带 is_error（如 USER_REJECTED: …）
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
| `Hub.start(config)` | 启动；`wsAddr` / `wsUrl` 为实际监听地址 |
| `shutdown()` | 关闭 App 连接与后台任务；之后调用抛 `HubError('SHUTDOWN')` |
| `apps()` / `tools(filter?)` / `resources()` / `overview(appId)` | 查询快照 |
| `callTool(req)` / `cancelCall(callId)` | 调用与取消（`req.timeout` 毫秒） |
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
