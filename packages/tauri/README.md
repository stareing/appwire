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

- `createTauriAppMcp(options)` - returns an `AppMcp` (same interface as `@app-mcp/web`); accepts an explicit `bridge`.
- `isTauri(target?)` - whether the page runs in a Tauri v2 webview.
- `getTauriBridge(target?)` - the injected bridge, or `undefined` if missing or incompatible.
- `TAURI_OP_COMMAND` (`'plugin:app-mcp|op'`), `TAURI_DISPATCH_FN`, `BRIDGE_VERSION` - protocol constants.
- Types: `TauriAppMcpOptions`, `AppMcpBridge`, `HelloReply`, `MainEvent`, `OpReply`, `RendererOp`.

See the [AppWire README](https://github.com/stareing/appwire#readme) for how the local Hub and MCP clients fit together.

## License

MIT OR Apache-2.0
