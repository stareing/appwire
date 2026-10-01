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

Register the annotated tools at runtime through the generated virtual module (add
`/// <reference types="@app-mcp/build/client" />` to `src/vite-env.d.ts` for its types):

```ts
import { registerAnnotated } from 'virtual:app-mcp/annotated'
import { appMcp } from './mcp/app' // createAppMcp(...) from @app-mcp/web

registerAnnotated(appMcp)
```

Static tools are defined once with `defineStaticTool` from `@app-mcp/build/define` (safe to import in the
browser) and reused at runtime to register the matching handler, keeping manifest and runtime in sync.

## API

- `appMcp(options)` (also the default export) - the Vite plugin. Options: `appId`, `name`, `version`,
  `description`, `overview`, `launch`, `wake`, `resources`, `staticTools`, `annotations`, `outFile`, `writeTo`.
  `annotations` is `true` or `{ include, exclude, tsconfig, outputSchema }`.
- `@app-mcp/build/define` - `defineStaticTool`, `defineStaticTools`, `defineOverview`, `validateOverview`
  (no Node or Vite dependencies).
- `generateManifest`, `validateManifest`, `toInputSchema`, `toOutputSchema`, `normalizeWake`, `ManifestError` - manifest helpers.
- `@app-mcp/build/annotations` - `scanAnnotations`, `createAnnotationScanner`, `generateAnnotatedModule`.
- `@app-mcp/build/client` - types for `virtual:app-mcp/annotated` (`registerAnnotated`, `annotatedTools`).

See the [AppWire README](https://github.com/stareing/appwire#readme) for how the local Hub and MCP clients fit together.

## License

MIT OR Apache-2.0
