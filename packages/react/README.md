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
    risk: 'payment',                // the Hub asks the user for approval
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

Handlers always see the latest closure; changes to `description`, `input`, `risk`, `activation` or
`enabled` are sent to the Hub as updates without re-registering. Without a provider the hooks are no-ops.
A tool can also be loaded lazily: pass `load: () => import('./checkout')` instead of `handler`.

## API

- `AppMcpProvider` (`value: AppMcp`) - provides the instance created with `@app-mcp/web`.
- `useTool(name, definition)` - registers a tool while mounted; returns the `ToolHandle` (or `null` on the first render).
- `useResource(name, { description, read, mimeType?, deps? })` - exposes UI state as an MCP resource.
- `ToolScope` (`name`) - lifecycle boundary; unmounting removes all tools and resources registered below it.
- `useHold(active = true)` - prevents automatic sleep while mounted (`lifecycle.mode` `idle` / `on-demand`).
- `useConnectionState()` - subscribes to the Hub connection state (re-renders on change).
- `useAppMcp()` - returns the provided `AppMcp` instance.
- Re-exported types: `ToolDefinition`, `LazyToolDefinition`, `ToolHandle`, `ToolResult`, `ResourceDefinition`,
  `Risk`, `Activation`, `ConnectionState` and more from `@app-mcp/web`.

See the [AppWire README](https://github.com/stareing/appwire#readme) for how the local Hub and MCP clients fit together.

## License

MIT OR Apache-2.0
