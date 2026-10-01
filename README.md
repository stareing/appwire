# app-mcp

**English** · [简体中文](docs/README.zh-CN.md)

> **Everything is a Tool.**
> Apps are capabilities. Interfaces are declarations. A call is a wake-up.

app-mcp lets any app — web, desktop or mobile — expose its real actions to AI models as
[MCP](https://modelcontextprotocol.io) tools, without changing how people use the app and without
the model scraping screens. One Hub connects every app on the device to any agent: Claude Code and
other MCP clients, your own LLM loop, or an assistant a device vendor embeds.

## Philosophy

Unix says *everything is a file*: devices, pipes and processes share one interface — `open`, `read`,
`write`. Plugin systems say *everything is a plugin*: features are code loaded into a host.

app-mcp says **everything is a tool**. A button, a form, a menu command, a store action, an OS
capability, an existing MCP server — each is expressed the same way: a name, an input schema, a risk
level and a handler. A model needs only three verbs: **list, call, read**.

A plugin moves code *into* the host. A tool is the opposite: the code stays in the app, the app
declares what it can do, and the model orchestrates.

### Eight principles

1. **Declare where the action lives.** Capabilities are declared where they already are — a React
   hook, an HTML attribute, a doc comment, a state store, a native `ToolSpec`. No second description
   to maintain; when the code changes, the tool changes.
2. **Declare, don't parse.** No screenshots, no DOM scraping, no guessing which button is clickable.
   The app states what it can do; the model receives intent, not pixels. UI inspection
   (`@app-mcp/inspect`) is an opt-in fallback only.
3. **Tools live and die with the interface.** Open a tab and its tools appear; close it and they
   disappear; an empty cart has no `checkout`. The model always sees what can be done *now*.
4. **Sleep when idle, wake on call.** Idle apps drop their connection and release threads; nothing
   pins a process in memory. When needed, the Hub wakes the app through the platform's native
   activation and resumes in one round trip. A connection is a means, never a burden.
5. **One hub, every endpoint.** One Hub serves web, desktop and mobile apps, and speaks MCP, OpenAI,
   Anthropic and Gemini tool formats — or embeds directly into a vendor's own agent. Integrate once,
   use everywhere.
6. **Compatible, therefore a superset.** WebMCP, App Intents, AppFunctions and Windows App Actions
   can all be consumed and generated. We don't compete with standards; we connect them.
7. **Description is not authorization.** An overview tells the model what an app is for; it grants
   nothing. Risk levels and approvals decide what runs — the human stays in control.
8. **Fix it at the source.** Solve a problem in the layer where it originates. No forwarding
   processes, wrapper scripts, monkeypatches or fallback conversions to paper over it.

## How it works

```mermaid
flowchart TD
  clients["MCP clients · your LLM loop · vendor agent"]
  hub["app-mcp Hub<br/>routing · overview · approval<br/>lifecycle: sleep / wake / lease"]
  clients -- "MCP (stdio · Streamable HTTP)<br/>tool-format export + dispatch · embedded API" --> hub
  hub -- "WebSocket (local)" --> web["Web SDK<br/>(WASM core)"]
  hub -- "WebSocket (local)" --> desktop["Desktop SDKs<br/>Rust · C/C++ · C# · Python"]
  hub -- "WebSocket (local)" --> mobile["Mobile SDKs<br/>Kotlin · Swift · Dart/Flutter"]
  hub -- "WebSocket (local)" --> node["Node / Electron"]
  hub -- "child process" --> upstream["existing MCP servers"]
  subgraph core["one Rust sans-IO core shared by every language"]
    web
    desktop
    mobile
    node
  end
```

- **App SDKs** register tools and resources; a single Rust core (`crates/core`) implements the
  protocol, so behavior is identical in every language.
- **The Hub** (`crates/hub`) aggregates apps and upstream MCP servers, routes calls to the right
  instance, attaches a short app overview on first contact, enforces approvals, and wakes sleeping
  apps. `app-mcp-host` is its command-line front end.
- **Static manifests** (`app-mcp.json`) let the Hub list an app's tools and wake it even when the app
  is not running.

## A taste

**React**

```tsx
useTool('cart.checkout', {
  description: 'Check out the current cart',
  risk: 'payment',
  input: z.object({ addressId: z.string() }),
  handler: ({ addressId }) => checkout(addressId),
})
```

**Plain HTML**

```html
<button data-mcp-tool="cart.clear" data-mcp-desc="Empty the cart">Clear</button>
```

**A doc comment** (with `@app-mcp/build`)

```ts
/** Estimate delivery days for a city. @mcp */
export function deliveryEstimate(city: string, express?: boolean) { … }
```

**Your own LLM loop** (embedded Hub, Node)

```ts
const hub = await Hub.start({})
const tools = hub.exportTools('anthropic')            // pass to the model
const results = await handleAnthropicToolUses(hub, response.content)
```

## Platforms and packages

| Where | Package | Notes |
|---|---|---|
| Web | `@app-mcp/web`, `@app-mcp/react`, `@app-mcp/dom`, `@app-mcp/store`, `@app-mcp/build` | WASM core; WebMCP polyfill/bridge; HTML attributes; Zustand / Redux / Pinia; compile-time `@mcp` |
| Node / Electron | `@app-mcp/node`, `@app-mcp/electron` | main process + renderer bridge |
| Rust (Tauri, egui…) | `crates/native` | direct dependency |
| C / C++ | `bindings/c`, `sdks/cpp` | stable C ABI (`app_mcp.h`) |
| C# (WPF, WinUI) | `sdks/dotnet` | P/Invoke; single-instance and protocol activation helpers |
| Kotlin / Android | `sdks/kotlin` | coroutines; `WakeReceiver` + expedited WorkManager |
| Swift (iOS, macOS) | `sdks/swift` | async/await; SwiftUI lifecycle modifier |
| Python | `sdks/python` | sync or asyncio handlers; Qt / Tk dispatchers; D-Bus wake |
| Dart / Flutter | `sdks/dart` | dart:ffi; `AppLifecycleListener` integration |
| Native intents | `crates/codegen` | generates App Intents, AppFunctions, Windows App Actions and typed interfaces |
| Agents / vendors | `crates/hub`, `@app-mcp/hub`, `bindings/hub-c`, `bindings/hub-uniffi` | embeddable Hub for Rust, Node, C/C#, Kotlin, Swift, Python |

## Try it

```bash
# build the Host and run it as a resident service (one process serves every MCP client)
cargo build -p app-mcp-host
target/debug/app-mcp-host serve            # or: app-mcp-host service install  (start at login)
target/debug/app-mcp-host status           # one-line summary
target/debug/app-mcp-host doctor           # something wrong? each check gives a verdict and a fix

# run the demo shop, then open it in a browser
pnpm --filter @app-mcp/example-shop dev
```

The Host serves everything on one port, `127.0.0.1:7717`: web apps connect to `/app` (WebSocket),
MCP clients use Streamable HTTP at `http://127.0.0.1:7717/mcp`, and `/healthz` reports the Host's
identity. Native apps connect over a per-user local socket (Unix domain socket / Windows named
pipe), which also serves MCP for agents that speak HTTP over local sockets. A lock file keeps one Host per user, and the actual endpoints are recorded in
`~/.app-mcp/run/endpoints.json`. This repository's `.mcp.json` points Claude Code at that endpoint;
restart the session, open the demo page, and ask Claude to operate the shop. When something does not
connect, `app-mcp-host doctor` checks the Host, lock, local socket permissions, which process holds
the port, Windows excluded port ranges, the token mode, `adb reverse`, and each app's state and last
error; SDK states carry machine-readable error codes (`spec/protocol.md` §10). See
[`crates/host/README.md`](crates/host/README.md) for configuration, the access token and other MCP
clients.

## Documentation

| Document | Content |
|---|---|
| [`spec/protocol.md`](spec/protocol.md) | SDK ↔ Hub protocol (authoritative) |
| [`spec/manifest.md`](spec/manifest.md) | static manifest `app-mcp.json` |
| [`spec/lifecycle.md`](spec/lifecycle.md) | app lifecycle: sleep, wake, lease, fast resume |
| [`spec/hub-api.md`](spec/hub-api.md) | embeddable Hub API and bindings |
| [`app-mcp-plan.md`](app-mcp-plan.md) | full design and roadmap (Chinese) |
| [`TASKS.md`](TASKS.md) | current status (Chinese) |

Other languages: [简体中文](docs/README.zh-CN.md)

## Status

Prototype (milestones M1–M2). The protocol, core, Hub and all language SDKs are implemented and
tested on Linux and Windows, with an Android device run; Apple platforms are verified on Linux only.
APIs may still change.

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), at your option.
