# @app-mcp/react

Part of [AppWire](https://github.com/stareing/appwire): expose your React components' actions and state as
MCP (Model Context Protocol) tools and resources for AI agents such as Claude, ChatGPT and Gemini, with
React hooks (`useTool`, `useResource`) that follow the component lifecycle.

A tool registered with `useTool` exists only while its component is mounted: the agent sees exactly the
actions the current screen supports, with no screen scraping or browser automation.

## Install

```bash
npm install @app-mcp/react @app-mcp/web
npm install zod   # optional: zod v4 schemas for tool input
```

Peer dependency: `react` `^18.2.0 || ^19.0.0`. `@app-mcp/web` provides `createAppMcp`, which connects the
page to the local AppWire Hub.

## Usage

Create one `AppMcp` instance and provide it at the root:

```tsx
import { AppMcpProvider } from '@app-mcp/react'
import { createAppMcp } from '@app-mcp/web'
import { createRoot } from 'react-dom/client'

const appMcp = createAppMcp({ appId: 'shop', appName: 'Demo Shop', appVersion: '0.1.0' })

createRoot(document.getElementById('root')!).render(
  <AppMcpProvider value={appMcp}>
    <App />
  </AppMcpProvider>,
)
```

Declare tools and resources next to the code that already does the work:

```tsx
import { ToolScope, useHold, useResource, useTool } from '@app-mcp/react'
import { z } from 'zod'

function CartPage({ items }: { items: CartItem[] }) {
  useHold(items.length > 0) // keep the connection awake while the cart is non-empty

  useResource('cart.state', {
    description: 'Current cart items and total',
    read: () => summarize(items),
    deps: [items], // notify subscribers when items change
  })

  useTool('cart.checkout', {
    description: 'Check out the current cart',
    input: z.object({ addressId: z.string() }),
    annotations: { destructiveHint: true, openWorldHint: true }, // standard MCP hints for the agent
    activation: 'foreground',
    enabled: items.length > 0,      // hidden from the agent when the cart is empty
    handler: async ({ addressId }) => ({ data: await checkout(addressId), stateHints: ['cart.state'] }),
  })

  return <Cart items={items} />
}

// Every tool and resource under a ToolScope is unregistered when it unmounts.
function CartTab({ items }: { items: CartItem[] }) {
  return (
    <ToolScope name="cart">
      <CartPage items={items} />
    </ToolScope>
  )
}
```

Handlers always see the latest closure; changes to `description`, `title`, `input`, `outputSchema`, `risk`,
`annotations`, `activation`, `enabled`, `surface`, `page` or `visibility` are sent to the Hub as updates without re-registering (`input`,
`outputSchema` and `annotations` written inline are compared by content, not by reference). Without a provider
the hooks are no-ops. A tool can also be loaded lazily: pass `load: () => import('./checkout')` instead of `handler`.

`useTool` takes the same optional declarations as `@app-mcp/web`'s `appMcp.tool` (rules in
[`spec/protocol.md` §3.2](https://github.com/stareing/appwire/blob/main/spec/protocol.md)):

- `annotations` - standard MCP tool annotations (`title`, `readOnlyHint`, `destructiveHint`, `idempotentHint`,
  `openWorldHint`), passed to the agent unchanged. With the legacy `risk` also given, declared fields win one by
  one and the missing ones are derived from `risk`. AppWire does not allow or block calls based on them: the agent
  decides, and confirmation of high-risk actions belongs in your app.
- `outputSchema` - schema of the result `data` (JSON Schema, zod v4 schema or an object with `toJSONSchema()`);
  a non-`object` root is wrapped by the Hub as `{ result: <schema> }`.
- `implements` - standard intents the tool implements (spec/intents.md), e.g. `['message.send@1']`; an inline array is
  compared by content, and removing it clears the declaration.
- `cache` - result caching (spec/protocol.md 3.6), e.g. `{ ttlMs: 30_000 }`: the Hub reuses results of a read-only tool
  for `ttlMs`; `scope: 'shared'` only for caller-independent data. An inline object is compared by content, removing it
  clears the declaration. `useResource` accepts `cache` too (a changed value re-registers the resource).
- `deprecated` - deprecation notice (spec/protocol.md 3.7), e.g. `{ message: 'Use cart.add2', replacement: 'cart.add2' }`;
  the tool stays callable, and breaking changes should use a new tool name (spec/manifest.md section 6). An inline
  object is compared by content, removing it clears the declaration.
- `undoable` - `true` tells agents the tool's results may carry `undo` (spec/protocol.md 3.8); handlers return
  `{ data, undo: { tool: 'cart.remove', arguments: { id }, label: 'Remove it again' } }` to give the inverse operation,
  which agents can run once via `apps.undo`. Changing or removing `undoable` calls `update`.
- Handlers may return `{ data, stateHints?, status?, stateResource?, summary?, annotations? }` with `status` one of
  `'done' | 'pending' | 'partial' | 'noop'`. It is unpacked only if it has a `data` key and every other key is one
  of these with a valid value; any other value is returned as `data` as a whole. Returning nothing (`undefined` /
  `null`, no `summary`) makes the Hub give the model the fixed text "已完成" ("done").

### View tools, layers and navigation

Tools can declare whether they depend on the UI ([`spec/protocol.md` §3.4](https://github.com/stareing/appwire/blob/main/spec/protocol.md)):
`surface: 'view'` tools are exposed only while their part of the UI is really visible and on top; `page` names the page
they live on, so the Hub can navigate there when an agent calls a tool from another page.

```tsx
function CartPage() {
  const ref = useRef<HTMLDivElement>(null)
  return (
    <div ref={ref}>
      {/* tools below default to page "cart", surface "view", anchored at the page root */}
      <ToolScope name="cart" page="cart" surface="view" anchor={ref}>
        <CartView />
        {confirming && (
          <ToolLayer name="Confirm checkout">   {/* lower-layer view tools pause while it is open */}
            <ConfirmDialog />
          </ToolLayer>
        )}
      </ToolScope>
    </div>
  )
}

function Root() {
  // React Router: page name -> path from the route table (routes with an `id` are pages)
  useRouterNavigation({ navigate: useNavigate(), pages: routes })
  return <Outlet />
}
```

- Visibility gating (done by `@app-mcp/web`): the anchor is mounted, rendered (`checkVisibility`), in the viewport
  (`IntersectionObserver`), not `hidden` / `inert` / behind an open modal `<dialog>`, the document is visible
  (`visibilityState`), and no `<ToolLayer>` is open above it. A gated tool appears to the Hub as disabled; opt out with
  `visibility: 'always'`. This covers keep-alive routes and hidden tab panels that stay mounted.
- `<ToolLayer name open? anchor? surface?>` pushes a layer while mounted and `open` (default `true`); tools inside default to
  `surface: 'view'` and do not inherit the outer `page` (a dialog is not a navigation target). While a layer is open, the
  Hub's navigation requests are refused with `NAVIGATION_DENIED` (the user is interacting with it).
- `useRouterNavigation({ navigate, pages, guard?, whileLayerOpen?, settleMs? })` works with React Router's `useNavigate()`
  or any `navigate(path)` function; `pages` is `{ name: '/orders/:id' }` or a route table. Params fill the path, the rest
  becomes the query string. Unknown pages and missing params fail with `NAVIGATION_FAILED`; `guard` returning `false` or
  a message refuses. `useNavigationHandler(handler, options?)` is the generic form. Mount it in the root component so the
  capability is declared at handshake.

## API

- `AppMcpProvider` (`value: AppMcp`) - provides the instance created with `@app-mcp/web`.
- `useTool(name, definition)` - registers a tool while mounted; returns the `ToolHandle` (or `null` on the first render).
- `useResource(name, { description, read, mimeType?, deps? })` - exposes UI state as an MCP resource.
- `ToolScope` (`name`, `anchor?`, `page?`, `surface?`, `visibility?`) - lifecycle boundary; unmounting removes all tools
  and resources registered below it. The optional props are defaults for the tools below it.
- `ToolLayer` (`name`, `open?`, `anchor?`, `surface?`) - a dialog / drawer / modal layer; lower-layer `view` tools pause
  while it is open.
- `useRouterNavigation(options)` / `useNavigationHandler(handler, options?)` - handle the Hub's page navigation requests.
- `useHold(active = true)` - prevents automatic sleep while mounted (`lifecycle.mode` `idle` / `on-demand`).
- Events (spec/protocol.md 3.5): there is no dedicated hook; declare once at startup and emit from anywhere through the
  client, e.g. `appMcp.declareEvent({ name: 'order.shipped', description: 'An order was shipped' })` next to
  `createAppMcp`, then `useAppMcp().emitEvent('order.shipped', { orderId })` in an effect or handler (returns `false`
  while not connected; the event is dropped).
- `useBusy(active)` - declares that the user is working in the app while `active` is true (spec/protocol.md 5.3), e.g.
  `useBusy(focused)` in an editor; write calls are then rejected or queued according to `busyPolicy`. Each active hook
  holds an `appMcp.beginBusy()` scope (reference-counted), so the app is busy while any of them is active; it does not
  clear an explicit `appMcp.setBusy(true)`.
- `useConnectionState()` - subscribes to the Hub connection state (re-renders on change).
- `useAppMcp()` - returns the provided `AppMcp` instance.
- Re-exported types: `ToolDefinition`, `LazyToolDefinition`, `ToolHandle`, `ToolResult`, `ResourceDefinition`,
  `Risk`, `Activation`, `ConnectionState` and more from `@app-mcp/web`, including the declaration and result contract
  types (`ToolAnnotations`, `ContentAnnotations`, `ResultStatus`, `ToolResultEnvelope`, `InputDefinition`,
  `OutputDefinition`) and error types (`ErrorKind`, `UserActionReason`, `UserActionRequiredOptions`). A handler may
  return a `ToolResultEnvelope` (`{ data, status?, summary?, stateResource?, stateHints?, annotations? }`); runtime
  values such as `ToolCallError` (including `ToolCallError.userActionRequired`) are imported from `@app-mcp/web`.

See the [AppWire README](https://github.com/stareing/appwire#readme) for how the local Hub and MCP clients fit together.

## License

MIT OR Apache-2.0
