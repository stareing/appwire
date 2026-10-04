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
  risk: 'destructive', // legacy form: the agent sees destructiveHint: true
  handler: ({ id }) => {
    if (!notes.has(id)) throw new ToolCallError('INVALID_INPUT', `No note ${id}`)
    notes.delete(id)
    return { data: { deleted: id }, stateHints: ['notes.list'] }
  },
})

appMcp.resource('notes.list', { description: 'All notes', read: () => notes.all() })

appMcp.onStateChange((state) => console.log('app-mcp:', state.status))
```

### Tool declarations and results (optional)

Everything below is optional; without it tools and results behave exactly as before. The authoritative
rules are in [`spec/protocol.md` §3.2](https://github.com/stareing/appwire/blob/main/spec/protocol.md).

```ts
appMcp.tool('notes.archive', {
  description: 'Archive all notes older than `days`',
  input: z.object({ days: z.number().int().positive() }),
  annotations: { destructiveHint: false, idempotentHint: true },
  outputSchema: z.object({ archived: z.number() }),
  handler: ({ days }) => {
    const archived = notes.archiveOlderThan(days)
    if (archived === 0) return { data: { archived }, status: 'noop', summary: 'Nothing to archive' }
    return { data: { archived }, stateHints: ['notes.list'] }
  },
})
```

- `annotations` - standard MCP tool annotations (`title`, `readOnlyHint`, `destructiveHint`,
  `idempotentHint`, `openWorldHint`), passed to the agent unchanged. With `risk` also given, declared fields
  win one by one and the missing ones are derived from `risk` (the legacy form). AppWire does not allow or
  block calls based on them: the agent decides, and confirmation of high-risk actions belongs in your app.
- `outputSchema` - schema of the result `data`: a JSON Schema object, a zod v4 schema (converted in output
  mode, not validated) or an object with `toJSONSchema()`. A non-`object` root is wrapped by the Hub as
  `{ result: <schema> }`.
- Structured result (`ToolResultEnvelope`) - a returned object is unpacked only if it has a `data` key and every
  other key is one of `stateHints`, `status` (`'done' | 'pending' | 'partial' | 'noop'`, default `'done'`),
  `stateResource` (resource to read for the follow-up state of `pending`), `summary` (one sentence placed
  before the data) or `annotations` (MCP content annotations: `audience`, `priority`, `lastModified`), each with
  a valid value; any other value is returned as `data` as a whole.
- No return value (`undefined` / `null`, no `summary`, status `done`): the Hub gives the model the fixed text
  "已完成" ("done") instead of `null`.
- `handle.update({ annotations: undefined })` / `{ outputSchema: undefined }` clears a declaration.
- `implements: ['message.send@1']` declares the standard intents (spec/intents.md) a tool implements, so agents can find it
  via the built-in `apps.intents`; a malformed entry makes registration throw, an unknown verb only logs a warning.
  `handle.update({ implements: undefined })` clears it.
- `cache: { ttlMs: 30_000, scope?: 'private' | 'shared' }` (spec/protocol.md 3.6) lets the Hub reuse a result for
  `ttlMs` without calling or waking the App. It only applies to read-only tools (on a write tool it is ignored with a
  warning); use `scope: 'shared'` only for data that does not depend on the caller (default `private`). Resources take
  `cache` too. `ttlMs` must be an integer in 1..=86400000, otherwise registration / update throws `INVALID_CONFIG`;
  `handle.update({ cache: undefined })` clears it.
- `deprecated: { message: 'Use orders.search: supports paging', replacement?: 'orders.search', until?: '2027-06-30' }`
  (spec/protocol.md 3.7) marks a tool as deprecated. The Hub shows the message to agents; the tool stays listed and
  callable. For a breaking change register a new tool name and deprecate the old one (spec/manifest.md section 6).
  `message` must be 1..=500 characters, `replacement` a valid local tool name other than the tool itself, `until` a
  `YYYY-MM-DD` date, otherwise registration / update throws `INVALID_CONFIG`; `handle.update({ deprecated: undefined })`
  clears it.
- Resources take optional `annotations` too (MCP content annotations, shown on the resource in MCP
  `resources/list`): `appMcp.resource('notes.list', { description, annotations: { audience: ['user'], priority: 0.5 }, read })`.
  A `read` that throws `ToolCallError` (including `ToolCallError.userActionRequired`) fails the read with that kind
  and details, exactly like a tool handler.

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
  Electron), `instanceTitle`, `token` / `onPaired`, `autoStart`, `keepAlive`, `lifecycle`, `onIdleExit`,
  `heartbeat`, `callDedup` (`{ ttlMs?, maxEntries? }`, default 300000 ms / 64 entries, either 0 turns it off: a
  repeated `callId` within the TTL gets the first result replayed instead of running the handler again; each hit is
  logged as a warning), `registerName` / `nameInstance` (addressing by name, spec/naming.md: register the app with
  the system name service so the Hub - `app-mcp-host serve --name-service` - dials it on demand and the system starts
  it when not running; Linux uses the D-Bus name `dev.appmcp.App.<appId>`, Windows a per-app named pipe, both
  registered once with `app-mcp-host app install --app-id <id> --exec <program>`; other platforms ignore it with a log
  line. Usually combined with `lifecycle: { mode: 'on-demand', residency: 'exit-when-idle' }` and an `onIdleExit` that
  exits. `nameInstance` (`[a-z][a-z0-9-]{0,31}`, not `'default'`) additionally registers `appmcp://<appId>/<instance>`;
  an invalid value throws).
- Call scheduling (spec/protocol.md 5.3, local to the SDK, not sent to the Host): `maxConcurrentCalls` (default 1) caps
  calls running at once and `maxQueuedCalls` (default 64, 0 = unlimited) caps calls waiting; when the queue is full a new
  call fails with `RATE_LIMITED` (`data: { scope: 'queue', limit }`). Tool options `concurrency` (per-tool limit; absent
  / 0 = only `maxConcurrentCalls` applies) and `exclusive` (a group name; tools in the same group run one at a time, e.g.
  write tools on the same document) add per-tool rules; `update({ concurrency: undefined, exclusive: undefined })`
  clears them.
- User is busy (spec/protocol.md 5.3, local to the SDK): `setBusy(true / false)` declares that the user is working in the
  app right now (the app decides when, e.g. an editor has focus or a drag is in progress); `isBusy()` reads it. While
  busy, write calls (tools whose effective annotations are not `readOnlyHint: true`) follow `busyPolicy`: `'reject'`
  (default) fails them with `RATE_LIMITED` (`data: { scope: 'busy' }`; not executed, the same `callId` can be retried),
  `'queue'` holds them and runs them in arrival order after `setBusy(false)`. Read-only calls and calls already running
  are unaffected. `setBusyPolicy(policy)` changes the policy at runtime (an invalid value throws). `beginBusy()` opens a
  nestable, reference-counted busy scope (`release()` ends it); the effective state is the explicit switch OR any open
  scope, and neither clears the other.
- Events (spec/protocol.md 3.5): `declareEvent({ name, description, payloadSchema? })` declares an event the app can emit
  (same name replaces; synced to the Host when connected, otherwise on the next handshake - it never connects by
  itself); `emitEvent(name, payload?)` sends it and returns `true` when connected, or drops it and returns `false`
  while not connected (dormant, disconnected, reconnecting): no buffering, no wake-up, no idle-timer reset. Use a
  resource (`notifyChanged`) for state that must arrive. Local errors throw and send nothing: an undeclared or invalid
  name (`code: 'INVALID_NAME'`), a payload that is not a JSON object, cannot be serialized or exceeds 8 KiB
  (`code: 'INVALID_JSON'`). `removeEvent(name)` withdraws a declaration. Agents subscribe with the built-in tools
  `apps.events.subscribe` / `apps.events` (spec/hub-api.md 3.17). Static `events` in the manifest are only for listing
  while the app is not running; the SDK still has to declare them.
- Handler context: `callId`, `signal`, `hold()`, `progress()`, and `idempotencyKey` (the agent's idempotency key, passed
  through verbatim and stable across retries; absent when the agent gave none - see spec/protocol.md 3.3). Use it as a
  business-level dedup key or forward it to your backend.
- `AppMcp` - `tool`, `resource`, `scope`, `state`, `onStateChange`, `dispose`, plus Node-specific
  `start`, `setVisibility`, `handleWake`, `wake`, `connectNow`, `sleep`, `hold`, `onIdleExit`, `token`.
- Navigation (spec/protocol.md 3.4): tool options `surface` (`'app'` default / `'view'`: only registered while its screen
  is visible and topmost) and `page` (the page the tool lives on). `onNavigate: ({ page, params }) => …` (or
  `appMcp.setNavigationHandler(handler | null)`, set before connecting) lets the Hub switch pages before calling a tool
  that is not on the current page; throw `ToolCallError.navigationDenied(message)` to refuse, any other error fails it.
- In the background (`setVisibility('hidden' | 'frozen')`): navigations that need the foreground return
  `USER_ACTION_REQUIRED` (`reason: "foreground"`) immediately unless `navigateInBackground` is true (option or
  `appMcp.setNavigateInBackground(enabled)`; the native default is `true` on desktop, where the app can raise its own
  window, and `false` on mobile). With it on, the handler decides and may throw
  `ToolCallError.userActionRequired(message, { reason: 'foreground', uri })` (e.g. after posting a notification). Make
  capabilities that must work in the background `app` tools, or give a `view` tool `backgroundTool: '<app tool name>'`
  for the Hub to call instead; see spec/protocol.md 3.4 ("background and foreground").
- `ToolCallError(kind, message, details?)` - throw from a handler to return a specific error kind.
- `ToolCallError.userActionRequired(message, { reason?, uri? })` - the user must act first (login expired, OS permission
  missing, app must be in the foreground, in-app confirmation); the agent receives `USER_ACTION_REQUIRED` and relays `message`.
- `loadNativeBinding`, `nativeFileName` - low-level access to the native module.
- Types: `ToolDefinition`, `LazyToolDefinition`, `ResourceDefinition`, `Risk`, `Activation`,
  `ToolAnnotations`, `OutputDefinition`, `ToolResultEnvelope`, `ResultStatus`, `ContentAnnotations`,
  `ConnectionState`, `LifecycleOptions`, `NodeAppMcpOptions` and more.

For Electron apps, use [`@app-mcp/electron`](https://www.npmjs.com/package/@app-mcp/electron) on top of this package.

See the [AppWire README](https://github.com/stareing/appwire#readme) for how the local Hub and MCP clients fit together.

## License

MIT OR Apache-2.0
