# @app-mcp/electron

Part of [AppWire](https://github.com/stareing/appwire): expose your Electron app's actions, from both the
main process and renderer pages, as MCP (Model Context Protocol) tools for AI agents such as Claude,
ChatGPT and Gemini. The main process connects to the local AppWire Hub; renderer tools are bridged over IPC.

## Install

```bash
npm install @app-mcp/electron @app-mcp/node
```

Peer dependency: `electron` `>=30`. Page code can keep using `@app-mcp/web` or `@app-mcp/react` unchanged.

## Usage

Main process: create the native client (`clientKind: 'hybrid'`), accept renderer registrations, and
register native tools directly:

```ts
import { app, BrowserWindow, ipcMain } from 'electron'
import { createAppMcp } from '@app-mcp/node'
import { attachAppMcp, attachLifecycle } from '@app-mcp/electron/main'

const appMcp = createAppMcp({ appId: 'shop', appName: 'Demo Shop', clientKind: 'hybrid' })
attachAppMcp({ appMcp, ipcMain })

appMcp.tool('app.quit', { description: 'Quit the application', handler: () => app.quit() })

// Optional: wake on OS activation and report window visibility.
const win = new BrowserWindow({ webPreferences: { preload: PRELOAD_PATH } })
attachLifecycle({ appMcp, app, window: win })
```

Preload script: expose the minimal bridge (only the two app-mcp IPC channels):

```ts
import { contextBridge, ipcRenderer } from 'electron'
import { exposeAppMcpBridge } from '@app-mcp/electron/preload'

exposeAppMcpBridge(contextBridge, ipcRenderer)
```

Renderer: register page tools; they are forwarded to the main process and run in the page:

```ts
import { createRendererAppMcp } from '@app-mcp/electron/renderer'

export const appMcp = createRendererAppMcp({ appId: 'shop', appName: 'Demo Shop' })
appMcp.tool('cart.clear', { description: 'Empty the cart', handler: () => cart.clear() })
```

`createAppMcp` from `@app-mcp/web` also detects the bridge automatically, so `@app-mcp/react` works as is.
Optional tool declarations and structured results ([`spec/protocol.md` §3.2](https://github.com/stareing/appwire/blob/main/spec/protocol.md))
work on both sides: main-process tools use them as documented in
[`@app-mcp/node`](https://www.npmjs.com/package/@app-mcp/node), and renderer tools may declare `annotations`
(standard MCP hints, passed to the agent unchanged; AppWire does not gate calls on them) and `outputSchema`, and
return `{ data, stateHints?, status?, stateResource?, summary?, annotations? }`. The bridge forwards all of these to
the main process unchanged, and `handle.update({ annotations: undefined })` clears a declaration.
Renderer tools may also declare `implements: ['message.send@1']` (standard intents, spec/intents.md), forwarded the same way.
Renderer tools and resources may declare `cache: { ttlMs, scope? }` (result caching, spec/protocol.md 3.6; read-only tools
only, `scope: 'shared'` only for caller-independent data), forwarded the same way; omitting it in an update clears it.
Renderer tools may declare `deprecated: { message, replacement?, until? }` (spec/protocol.md 3.7) the same way; a
deprecated tool stays callable, and breaking changes should use a new tool name (spec/manifest.md section 6).
Renderer tools may declare `undoable: true` and their handlers may return `{ data, undo: { tool, arguments?, label? } }`
(spec/protocol.md 3.8), forwarded the same way; an invalid `undo` is dropped by the native core with a warning.
Identity and connection belong to the main process: `appId`, `appName` and `hostUrl` are ignored in the page.
Each webContents gets its own scope, unregistered on reload, navigation, destroy or renderer crash
(in-flight calls fail with `APP_DISCONNECTED`).
Page resources declared with `realtime: true` are registered as realtime in the main process too (subscriptions keep the
connection alive and changes while dormant reconnect to push; default `false`). Resource `annotations` (MCP content
annotations) are forwarded the same way, and a page `read` that throws `ToolCallError` (including
`ToolCallError.userActionRequired`) reaches the Hub with its kind and details (`reason` / `uri`).

Lifecycle: the main-process client is created by `@app-mcp/node`, so its defaults apply (mode `persistent`;
`heartbeat`, `lifecycle.hostAbsentRetries` / `mergeWindowMs` / `sleepOnBackground` / `legacyTimers` as documented there).
This package does not change the mode: `idle` / `on-demand` is only safe with a wake path, i.e. a single-instance lock
(`app.requestSingleInstanceLock()`) plus `attachLifecycle` (handles `second-instance` / `open-url`) and a `lifecycle.wake`
descriptor, as in the `attachLifecycle` example in `main.ts`.

## API

- `@app-mcp/electron/main`
  - `attachAppMcp({ appMcp, ipcMain, webContents?, logger?, navigation?, raiseWindow? })` - bridges renderer registrations; returns
    `{ sessionCount, dispose() }`. `navigation: true` declares navigation support and forwards the Hub's page navigation
    requests to the renderer that enabled navigation most recently (`appMcp.setNavigationHandler` in the page, or
    `attachBridgeNavigation`; without `navigation: true`, the main process may call
    `appMcp.setNavigationHandler` itself, e.g. to switch windows). Renderer tool `surface` / `page` / `backgroundTool`
    and the call-scheduling options `concurrency` / `exclusive` (spec/protocol.md 5.3) are forwarded, and a page handler throwing `ToolCallError.userActionRequired(message, { reason?, uri? })` replies
    `USER_ACTION_REQUIRED`. The renderer's busy state (`setBusy` / `beginBusy`, spec/protocol.md 5.3) is recorded per
    webContents; while any page is busy the attachment holds an `appMcp.beginBusy()` scope, so it never clears an
    explicit `appMcp.setBusy(true)` in the main process. A page's declaration is dropped when it reloads, unloads or its
    webContents is destroyed. `busyPolicy` is configured on the main `appMcp`.
    Renderer events (`declareEvent` / `emitEvent` / `removeEvent`, spec/protocol.md 3.5) go over the bridge
    (`event.declare` / `event.emit` / `event.remove`) and are emitted by the main `appMcp`. Declarations are recorded per
    webContents and withdrawn when the page reloads, unloads or its webContents is destroyed (kept while another page
    still declares the same name). The page decides from its mirrored connection state: `emitEvent` returns `false`
    while the main client is not connected; local validation errors (`INVALID_NAME` / `INVALID_JSON`) throw in the page.
    In the background: with `navigation: true` the attachment sets `navigateInBackground` to
    whether `raiseWindow` is given. Pass
    `raiseWindow: (wc) => { const w = BrowserWindow.fromWebContents(wc); if (w?.isMinimized()) w.restore(); w?.show(); w?.focus() }`
    so a navigation brings the window to the front before the page switches routes; without it, navigations while the
    window is hidden (as reported by `attachLifecycle`) return `USER_ACTION_REQUIRED` (`reason: "foreground"`) at once.
    Capabilities that must work in the background should be `app` tools, or `view` tools with `backgroundTool`; see
    spec/protocol.md 3.4 ("background and foreground").
  - `attachLifecycle({ appMcp, app, argv?, window?, quitOnIdleExit?, onWake? })` - wires `second-instance`,
    `open-url`, window visibility and idle exit to the client lifecycle; returns an unsubscribe function.
- `@app-mcp/electron/preload`
  - `exposeAppMcpBridge(contextBridge, ipcRenderer, options?)` - exposes `window.appMcpBridge` (`key`, `resetOnPageHide`, `target`).
  - `createAppMcpBridge(ipcRenderer)` - creates the bridge without exposing it.
- `@app-mcp/electron/renderer`
  - `createRendererAppMcp(options)` - an `AppMcp` (same interface as `@app-mcp/web`, including `setNavigationHandler` and
    `view` tool visibility gating in the page); throws if no bridge is found.
  - `getAppMcpBridge(key?)`, `createIpcTransport(key?)`.
  - `attachBridgeNavigation(bridge, ({ page, params }) => …)` - handle navigation requests in this page without an `AppMcp`
    instance (switch the route, then return; throw `ToolCallError` with kind `NAVIGATION_DENIED` to refuse); returns
    `{ ready, dispose() }`. Re-exported from `@app-mcp/web`, which `appMcp.setNavigationHandler` uses as well.

See the [AppWire README](https://github.com/stareing/appwire#readme) for how the local Hub and MCP clients fit together.

## License

MIT OR Apache-2.0
