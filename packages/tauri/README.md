# @app-mcp/tauri

Part of [AppWire](https://github.com/stareing/appwire): expose your Tauri v2 app's webview actions as MCP
(Model Context Protocol) tools for AI agents such as Claude, ChatGPT and Gemini, registered with the
Rust-side `tauri-plugin-app-mcp` client over Tauri IPC and merged with your Rust tools into one app.

## Install

```bash
npm install @app-mcp/tauri
cargo add tauri-plugin-app-mcp   # in src-tauri
```

Register the plugin in Rust and allow it in your window's capability (`"permissions": ["core:default", "app-mcp:default"]`):

```rust
use tauri_plugin_app_mcp::NativeConfig;

tauri::Builder::default()
    .plugin(tauri_plugin_app_mcp::init(NativeConfig::new("tauri-counter", "Tauri Counter")))
    .run(tauri::generate_context!())
```

The plugin injects a host IPC bridge (`window.appMcpBridge`) into every webview. Rust-side tools are
registered on `app.app_mcp()?.client()`; see the
[plugin README](https://github.com/stareing/appwire/tree/main/crates/tauri-plugin).

## Usage

```ts
import { createTauriAppMcp } from '@app-mcp/tauri'

export const appMcp = createTauriAppMcp({ appId: 'tauri-counter', appName: 'Tauri Counter' })

let count = 0
appMcp.tool<{ by?: number }, { count: number }>('counter.increment', {
  description: 'Increase the counter by `by` (default 1) and return the new value',
  input: { type: 'object', properties: { by: { type: 'integer', minimum: 1 } } },
  handler: ({ by = 1 }) => ({ count: (count += by) }),
})

appMcp.tool('counter.get', { description: 'Read the current counter', risk: 'read', handler: () => ({ count }) })
```

Page tools accept the same optional declarations as `@app-mcp/web`
([`spec/protocol.md` §3.2](https://github.com/stareing/appwire/blob/main/spec/protocol.md)): `annotations`
(standard MCP `title` / `readOnlyHint` / `destructiveHint` / `idempotentHint` / `openWorldHint`; declared fields win
one by one over the legacy `risk`, passed to the agent unchanged, never used by AppWire to allow or block calls) and
`outputSchema` (schema of the result `data`; a non-`object` root is wrapped by the Hub as `{ result: <schema> }`).
Handlers may return `{ data, stateHints?, status?, stateResource?, summary?, annotations? }` (`status`:
`'done' | 'pending' | 'partial' | 'noop'`); it is unpacked only if it has a `data` key and every other key is one of
these with a valid value, otherwise the whole value is `data`. Returning nothing (no `summary`) makes the Hub give the model the fixed text "已完成" ("done").
The plugin forwards all of these to the Rust client unchanged.

You do not strictly need this package: `createAppMcp` from `@app-mcp/web` (and `@app-mcp/react`) detects
the plugin's bridge and routes registrations through Tauri `invoke` automatically, without loading WASM or
connecting to the Hub directly. Use `createTauriAppMcp` when the page always runs in Tauri and a missing
plugin should throw a clear error instead of falling back to a WebSocket connection.

Identity and connection are owned by the Rust side: `appId`, `appName` and `hostUrl` are ignored in the page.

## API

- `createTauriAppMcp(options)` - returns an `AppMcp` (same interface as `@app-mcp/web`, including `setNavigationHandler`
  and `view` tool visibility gating in the page); accepts an explicit `bridge`.
- `isTauri(target?)` - whether the page runs in a Tauri v2 webview.
- `getTauriBridge(target?)` - the injected bridge, or `undefined` if missing or incompatible.
- `attachTauriNavigation(({ page, params }) => …, bridge?)` - handle the Hub's page navigation requests in this page
  without an `AppMcp` instance (requires `Builder::page_navigation(true)` on the Rust side); returns `{ ready, dispose() }`.
  Usually `appMcp.setNavigationHandler(...)` (or `useRouterNavigation` from `@app-mcp/react`) is enough; both use
  `attachBridgeNavigation` from `@app-mcp/web`.
- In the background: the plugin restores / shows / focuses the page's window before forwarding a navigation on desktop
  (where the native runtime lets navigations through while hidden); on Android / iOS navigations that need the foreground
  return `USER_ACTION_REQUIRED` (`reason: "foreground"`) immediately. A page navigation handler may throw
  `ToolCallError.userActionRequired(message, { reason, uri })` itself. Capabilities that must work in the background
  should be `app` tools, or `view` tools with `backgroundTool` (the name of an `app` tool the Hub calls instead); see
  spec/protocol.md 3.4 ("background and foreground").
- Call scheduling (spec/protocol.md 5.3): page tool options `concurrency` / `exclusive` are forwarded to the plugin and
  scheduled by the native runtime (the limits `max_concurrent_calls` / `max_queued_calls` are set in the plugin's
  `NativeConfig` on the Rust side).
- Standard intents (spec/intents.md): the page tool option `implements` (e.g. `['link.open@1']`) is forwarded to the plugin;
  omitting it in an update clears it.
- Result caching (spec/protocol.md 3.6): page tools and resources may declare `cache: { ttlMs, scope? }`, forwarded to the
  plugin's native `ToolOptions.cache` / `ResourceOptions.cache` (read-only tools only; `scope: 'shared'` only for
  caller-independent data); omitting it in an update clears it.
- Deprecation (spec/protocol.md 3.7): page tools may declare `deprecated: { message, replacement?, until? }`, forwarded to
  the plugin's native `ToolOptions.deprecated`; omitting it in an update clears it. A deprecated tool stays listed and
  callable; for a breaking change register a new tool name instead (spec/manifest.md section 6).
- Events (spec/protocol.md 3.5): `appMcp.declareEvent({ name, description, payloadSchema? })`,
  `emitEvent(name, payload?)` and `removeEvent(name)` go over the plugin bridge (`event.declare` / `event.emit` /
  `event.remove`) and are emitted by the Rust-side client. Declarations belong to the page and are withdrawn when it
  reloads or closes (kept while another page still declares the same name). `emitEvent` returns `false` while the
  client is not connected (the event is dropped); an undeclared name or a payload that is not a JSON object / exceeds
  8 KiB throws in the page (`code` `INVALID_NAME` / `INVALID_JSON`).
- User is busy (spec/protocol.md 5.3): `appMcp.setBusy(true / false)` declares that the user is working in this page
  (e.g. an editor has focus); `beginBusy()` opens a nestable, reference-counted scope (`release()` ends it, `useBusy`
  in `@app-mcp/react` uses it); `isBusy()` returns this page's effective state (the switch OR any open scope). The plugin records it per page, sets the
  client's busy state to the OR of all pages, and drops a page's declaration when it reloads or closes. While busy,
  write calls are rejected (`RATE_LIMITED`, `data: { scope: 'busy' }`) or queued according to `busy_policy` in the
  plugin's `NativeConfig` on the Rust side (the page has no `setBusyPolicy`); read-only calls are unaffected.
- `TAURI_OP_COMMAND` (`'plugin:app-mcp|op'`), `TAURI_DISPATCH_FN`, `BRIDGE_VERSION` - protocol constants.
- Types: `TauriAppMcpOptions`, `AppMcpBridge`, `HelloReply`, `MainEvent`, `OpReply`, `RendererOp`, `NavigationOp`, `NavigateEvent`.

See the [AppWire README](https://github.com/stareing/appwire#readme) for how the local Hub and MCP clients fit together.

## License

MIT OR Apache-2.0
