# @app-mcp/build

Part of [AppWire](https://github.com/stareing/appwire): a Vite plugin that generates MCP (Model Context
Protocol) tools and the `app-mcp.json` manifest at build time, from `@mcp` doc comments and static tool
definitions, so AI agents such as Claude, ChatGPT and Gemini can see your app's tools even before it opens.

## Install

```bash
npm install -D @app-mcp/build vite typescript
npm install @app-mcp/web
```

Peer dependencies: `vite` `^7.0.0 || ^8.0.0`, and `typescript` `>=5.0.0` (optional, required only for the
`annotations` option).

## Usage

Add the plugin to `vite.config.ts`:

```ts
import { appMcp } from '@app-mcp/build'
import { defineConfig } from 'vite'

export default defineConfig({
  plugins: [
    appMcp({
      appId: 'shop',
      name: 'Demo Shop',
      version: '0.1.0',
      launch: { web: 'http://localhost:5173/' },
      staticTools: './src/mcp/static-tools.ts', // default export: defineStaticTools([...])
      annotations: true,                          // scan src/**/*.{ts,tsx,mts,cts} for @mcp comments
      writeTo: 'app-mcp.json',                    // extra copy for `app-mcp-host --manifest`
    }),
  ],
})
```

The build output contains `.well-known/app-mcp.json` (change with `outFile`).

Declare a tool with a doc comment; the input JSON Schema is derived from the TypeScript parameter types:

```ts
/**
 * Estimate delivery days for a city
 * @mcp deliveryEstimate
 * @risk read
 * @activation headless
 * @param city Destination city
 * @param express Use express delivery
 */
export function deliveryEstimate(city: string, express?: boolean) {
  return { city, days: express ? 1 : 3 }
}
```

Optional declarations (all default to "not declared", see `spec/protocol.md` §3.2):

- **Tool annotations** — `@readOnly` (or `@readonly`), `@destructive`, `@idempotent`, `@openWorld` map to the
  standard MCP hints `readOnlyHint` / `destructiveHint` / `idempotentHint` / `openWorldHint`. A bare tag means
  `true`; `true` / `false` may be written explicitly. Declared hints win field by field over `@risk`; missing ones
  are derived from it. They are passed through to the agent unchanged — AppWire does not gate calls on them.
- **Output schema** — the return type (with `Promise` unwrapped, and the `data` type taken when the function returns a
  structured result `{ data, status?, summary?, … }`) becomes the tool's `outputSchema`. No return value, `any` /
  `unknown`, or a type that cannot be converted (with a warning) declares none; turn it off with
  `annotations: { outputSchema: false }`.

```ts
/**
 * Cancel an order
 * @mcp order.cancel
 * @destructive
 * @idempotent
 * @openWorld false
 */
export async function cancelOrder(orderId: string): Promise<{ refunded: boolean }> { /* ... */ }
```

Static tools accept the same declarations as `annotations` and `outputSchema` (JSON Schema, zod v4 converted in its
output shape, or an object with `toJSONSchema()`); both are validated and written to the manifest.
Static tools and page tools found by the route scanner may also declare `implements: ['message.send@1']`
(standard intents, spec/intents.md); the format (`<verb>@<major>`, at most 4, no duplicates) is checked like
`crates/manifest`, and vocabulary warnings come from `app-mcp-host validate`. `@mcp` JSDoc comments do not support it yet.
Tools (static and scanned page tools) and plugin `resources` may declare `cache: { ttlMs, scope? }` (result caching,
spec/protocol.md 3.6); it is written to the manifest after a format check like `crates/manifest` (`ttlMs` an integer
in 1..=86400000, `scope` `private` / `shared`), with a warning when the tool is not read-only (the Hub ignores it there).
Use `scope: 'shared'` only for caller-independent data.

Register the annotated tools at runtime through the generated virtual module (add
`/// <reference types="@app-mcp/build/client" />` to `src/vite-env.d.ts` for its types):

```ts
import { registerAnnotated } from 'virtual:app-mcp/annotated'
import { appMcp } from './mcp/app' // createAppMcp(...) from @app-mcp/web

registerAnnotated(appMcp)
```

Static tools are defined once with `defineStaticTool` from `@app-mcp/build/define` (safe to import in the
browser) and reused at runtime to register the matching handler, keeping manifest and runtime in sync.

### Pages (page directory)

The manifest's `pages` ([`spec/manifest.md` §2.3](https://github.com/stareing/appwire/blob/main/spec/manifest.md)) tells
the Hub which pages the app has, which tools live on each, and lets it navigate there when an agent calls a tool on
another page. Two sources, merged by page name (explicit wins; a missing `route` is taken from the scan):

```ts
appMcp({
  // scan route tables: React Router routes with an `id` / Vue Router routes with a `name` are pages
  routes: { file: 'src/routes.tsx', router: 'react-router' },
  // explicit definitions for pages the scan cannot determine statically
  pages: './src/mcp/pages.ts', // export default definePages([{ name: 'cart', tools: [...] }])
})
```

Scan rules (static only, no guessing): the route table is an array literal of route objects; a page's name, `path` and
the page description (`handle: { mcp: { title, description, navigable, activation, params } }` in React Router,
`meta: { mcp: {...} }` in Vue Router) must be literals. The page component (`element: <X />`, `Component`, `lazy` /
`component`, `() => import('./x')`) must come from a relative module; only that module is scanned (not the child
components it imports: a dialog's tools exist only while it is open and are not navigation targets). Its
`useTool('name', { ... })` / `<x>.tool('name', { ... })` calls become page tools; name, `description`, `title`, `risk`,
`activation`, `surface`, `page`, `annotations`, `input` and `outputSchema` must be literals (`input` as a JSON Schema
literal; zod schemas are runtime values). Anything else is a build error with its location, asking for an explicit
`definePage()`; explicitly declared pages are not scanned.

## Events

Static event declarations (spec/manifest.md 2.4) go into the manifest's `events` through the plugin option of the same
name, so agents can see and subscribe to them while the app is not running:

```ts
appMcp({
  appId: 'shop',
  name: 'Shop',
  events: [{ name: 'order.shipped', description: 'An order was shipped', payloadSchema: { type: 'object' } }],
})
```

Names follow the tool-name rule and must be unique within `events`; `description` must not be empty and
`payloadSchema` must be an object. The SDK does not read the manifest: the page still calls
`appMcp.declareEvent(...)` before `emitEvent(...)`.

## API

- `appMcp(options)` (also the default export) - the Vite plugin. Options: `appId`, `name`, `version`,
  `description`, `overview`, `launch`, `wake`, `resources`, `staticTools`, `annotations`, `pages`, `routes`,
  `outFile`, `writeTo`. `routes` is `{ file, router: 'react-router' | 'vue-router' }` or an array of them.
  `annotations` is `true` or `{ include, exclude, tsconfig, outputSchema }`.
- `@app-mcp/build/define` - `defineStaticTool`, `defineStaticTools`, `definePage`, `definePages`, `defineOverview`, `validateOverview`
  (no Node or Vite dependencies).
- `generateManifest(info, tools, options, pages)`, `validateManifest`, `loadPages`, `mergePages`, `toInputSchema`, `toOutputSchema`, `normalizeWake`, `ManifestError` - manifest helpers.
- `@app-mcp/build/annotations` - `scanAnnotations`, `createAnnotationScanner`, `generateAnnotatedModule`.
- `@app-mcp/build/client` - types for `virtual:app-mcp/annotated` (`registerAnnotated`, `annotatedTools`).

See the [AppWire README](https://github.com/stareing/appwire#readme) for how the local Hub and MCP clients fit together.

## License

MIT OR Apache-2.0
