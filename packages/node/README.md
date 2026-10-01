# @app-mcp/node

Part of [AppWire](https://github.com/stareing/appwire): expose your Node.js program's actions (CLIs, daemons,
Electron main processes) as MCP (Model Context Protocol) tools for AI agents such as Claude, ChatGPT and
Gemini, through a native (napi-rs) client that connects to the local AppWire Hub.

The API has the same shape as `@app-mcp/web` (`tool`, `resource`, `scope`, `onStateChange`, `dispose`,
`ToolCallError`), so the same registration code runs in the browser and in Node.

## Install

```bash
npm install @app-mcp/node
npm install zod   # optional: zod v4 schemas for tool input
```

The package ships a prebuilt native module (`native/*.node`) loaded for the current platform.

## Usage

```ts
import { createAppMcp, ToolCallError } from '@app-mcp/node'
import { z } from 'zod'

const appMcp = createAppMcp({ appId: 'notes', appName: 'Notes CLI', appVersion: '1.0.0' })

appMcp.tool('notes.add', {
  description: 'Add a note and return its id',
  input: z.object({ text: z.string().min(1) }),
  risk: 'write',
  handler: ({ text }) => ({ id: notes.add(text) }),
})

appMcp.tool('notes.delete', {
  description: 'Delete a note by id',
  input: z.object({ id: z.string() }),
  risk: 'destructive', // the Hub asks the user for approval
  handler: ({ id }) => {
    if (!notes.has(id)) throw new ToolCallError('INVALID_INPUT', `No note ${id}`)
    notes.delete(id)
    return { data: { deleted: id }, stateHints: ['notes.list'] }
  },
})

appMcp.resource('notes.list', { description: 'All notes', read: () => notes.all() })

appMcp.onStateChange((state) => console.log('app-mcp:', state.status))
```

By default the client connects immediately (`autoStart: true`) and keeps the process alive while
connected (`keepAlive: true`); call `appMcp.dispose()` to disconnect. The Hub endpoint is resolved from
`hostUrl`, then `APP_MCP_ENDPOINT`, then `~/.app-mcp/run/endpoints.json`, then the platform's local socket
(Unix domain socket / Windows named pipe), then `ws://127.0.0.1:7717/app`.

Sleep and wake: pass `lifecycle: { mode: 'idle' | 'on-demand', ... }`, forward OS activation arguments
with `appMcp.handleWake(process.argv)`, and decide in `onIdleExit` whether to exit (the SDK never exits
the process itself).

## API

- `createAppMcp(options)` - creates the client. Options include `appId`, `appName`, `appVersion`,
  `enabled`, `hostUrl`, `overview`, `maxConcurrentCalls`, `clientKind` (`'native'` or `'hybrid'` for
  Electron), `instanceTitle`, `token` / `onPaired`, `autoStart`, `keepAlive`, `lifecycle`, `onIdleExit`.
- `AppMcp` - `tool`, `resource`, `scope`, `state`, `onStateChange`, `dispose`, plus Node-specific
  `start`, `setVisibility`, `handleWake`, `wake`, `connectNow`, `sleep`, `hold`, `onIdleExit`, `token`.
- `ToolCallError(kind, message, details?)` - throw from a handler to return a specific error kind.
- `loadNativeBinding`, `nativeFileName` - low-level access to the native module.
- Types: `ToolDefinition`, `LazyToolDefinition`, `ResourceDefinition`, `Risk`, `Activation`,
  `ConnectionState`, `LifecycleOptions`, `NodeAppMcpOptions` and more.

For Electron apps, use [`@app-mcp/electron`](https://www.npmjs.com/package/@app-mcp/electron) on top of this package.

See the [AppWire README](https://github.com/stareing/appwire#readme) for how the local Hub and MCP clients fit together.

## License

MIT OR Apache-2.0
