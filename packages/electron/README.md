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
Identity and connection belong to the main process: `appId`, `appName` and `hostUrl` are ignored in the page.
Each webContents gets its own scope, unregistered on reload, navigation, destroy or renderer crash
(in-flight calls fail with `APP_DISCONNECTED`).
Page resources declared with `realtime: true` are registered as realtime in the main process too (subscriptions keep the
connection alive and changes while dormant reconnect to push; default `false`).

Lifecycle: the main-process client is created by `@app-mcp/node`, so its defaults apply (mode `persistent`;
`heartbeat`, `lifecycle.hostAbsentRetries` / `mergeWindowMs` / `sleepOnBackground` / `legacyTimers` as documented there).
This package does not change the mode: `idle` / `on-demand` is only safe with a wake path, i.e. a single-instance lock
(`app.requestSingleInstanceLock()`) plus `attachLifecycle` (handles `second-instance` / `open-url`) and a `lifecycle.wake`
descriptor, as in the `attachLifecycle` example in `main.ts`.

## API

- `@app-mcp/electron/main`
  - `attachAppMcp({ appMcp, ipcMain, webContents?, logger? })` - bridges renderer registrations; returns `{ sessionCount, dispose() }`.
  - `attachLifecycle({ appMcp, app, argv?, window?, quitOnIdleExit?, onWake? })` - wires `second-instance`,
    `open-url`, window visibility and idle exit to the client lifecycle; returns an unsubscribe function.
- `@app-mcp/electron/preload`
  - `exposeAppMcpBridge(contextBridge, ipcRenderer, options?)` - exposes `window.appMcpBridge` (`key`, `resetOnPageHide`, `target`).
  - `createAppMcpBridge(ipcRenderer)` - creates the bridge without exposing it.
- `@app-mcp/electron/renderer`
  - `createRendererAppMcp(options)` - an `AppMcp` (same interface as `@app-mcp/web`); throws if no bridge is found.
  - `getAppMcpBridge(key?)`, `createIpcTransport(key?)`.

See the [AppWire README](https://github.com/stareing/appwire#readme) for how the local Hub and MCP clients fit together.

## License

MIT OR Apache-2.0
