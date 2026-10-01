# @app-mcp/store

> Part of [AppWire](https://github.com/stareing/appwire): expose Zustand, Redux (including RTK) and Pinia store actions as MCP tools and state slices as MCP resources for AI agents such as Claude, ChatGPT and Gemini.

把状态库的 **action 暴露为 MCP 工具**、把 **state 切片暴露为资源**。支持 Zustand、Redux（含 RTK）、Pinia，
也可以通过通用核心 `exposeStore` 接入任何"`getState` + `subscribe`"形状的 store。

```
pnpm add @app-mcp/store @app-mcp/web
```

本包不在运行时 import zustand / redux / pinia / vue，只用结构类型描述需要的最小形状。

## 为什么挂在状态层

- **人和模型走同一条路径。** 按钮的 `onClick` 调用 `cart.add(item)`，模型调用 `cart.add` 工具时执行的也是这个 action。
  校验、埋点、乐观更新、持久化、撤销栈都只写一遍，不会出现"UI 能做但工具做不到"或"工具绕过了业务规则"。
- **不依赖 UI 是否挂载。** 工具在 store 创建时注册，跟页面上当前渲染了哪个组件无关；切换路由、折叠面板不会让工具时有时无。
  需要按条件开放的工具用 `enabled(state)` 表达——它由状态决定，而不是由某个按钮是否存在决定。
- **状态即上下文。** 资源直接从 state 选出，模型读到的就是界面渲染所用的同一份数据，调用工具后通过 `result` 选择器和
  `hints`（stateHints）立即拿到新状态，而不必"看屏幕"。
- **无 UI 耦合，容易测试。** 工具定义是纯数据 + 纯函数，可以在 Node 里用真实 store 测试，不需要 DOM。

## 用法

### Zustand

```ts
import { createAppMcp } from '@app-mcp/web'
import { exposeZustand } from '@app-mcp/store/zustand'
import { create } from 'zustand'
import { z } from 'zod'

export const useCart = create<CartState>()((set, get) => ({
  items: [],
  add: (item) => set({ items: [...get().items, item] }),
  setQty: (id, qty) => set({ items: get().items.map((i) => (i.id === id ? { ...i, qty } : i)) }),
  clear: () => set({ items: [] }),
}))

const appMcp = createAppMcp({ appId: 'shop', appName: '示例商城' })

const dispose = exposeZustand(appMcp, useCart, {
  namespace: 'cart',
  actions: {
    add: {                                   // 工具名 cart.add，调用 getState().add(input)
      description: '把商品加入购物车',
      input: z.object({ id: z.string(), qty: z.number().int().positive() }),
      result: (s) => ({ count: s.items.length }),
      hints: ['cart.items'],
    },
    'item.setQty': {                         // 工具名 cart.item.setQty，action 默认取最后一段 setQty
      description: '修改商品数量',
      input: z.object({ id: z.string(), qty: z.number().int().positive() }),
      args: (i) => [i.id, i.qty],            // 参数对象 → 位置参数
    },
    clear: {
      description: '清空购物车',
      risk: 'destructive',
      enabled: (s) => s.items.length > 0,   // 购物车为空时对模型不可见
    },
  },
  resources: {
    items: { description: '购物车商品列表', select: (s) => s.items },  // 资源名 cart.items
  },
})
```

### Redux / Redux Toolkit

```ts
import { exposeRedux } from '@app-mcp/store/redux'

exposeRedux(appMcp, store, {
  namespace: 'todos',
  actions: {
    add: {
      description: '新增待办',
      input: { type: 'object', properties: { text: { type: 'string' } }, required: ['text'] },
      creator: (i) => todosSlice.actions.add(i.text),   // dispatch(creator(...args))
    },
    fetch: {
      description: '从服务器同步待办',
      creator: fetchTodos,                              // createAsyncThunk：结果为 fulfilled 的 payload
      result: (s) => s.todos.items,
    },
    clear: {
      description: '清空',
      action: 'todos/clear',                            // 没有 creator 时：dispatch({ type, payload? })
      enabled: (s) => s.todos.items.length > 0,
    },
  },
  resources: {
    list: { description: '待办列表', select: (s) => s.todos.items },
  },
})
```

- 普通 action creator：`dispatch` 返回 action 本身，视为无返回值（默认结果 `{ ok: true }`）。
- thunk：结果为 thunk 的返回值；返回 Promise 时等待。
- `createAsyncThunk`：按 `unwrap()` 语义处理。rejected 时：
  - `rejectWithValue({ kind: 'INVALID_INPUT', message: '...', details })`（`kind` 为协议错误类别）→ 对应类别的 `ToolCallError`；
  - 抛出带 `code` 的错误且 `code` 是协议错误类别（如 `'UNAUTHORIZED'`）→ 对应类别的 `ToolCallError`；
  - 其他 → `HANDLER_ERROR`。

  注意：在 `createAsyncThunk` 里直接 `throw new ToolCallError(...)` 会被 RTK 序列化，`kind` 丢失（变成 `HANDLER_ERROR`）。
  请改用 `rejectWithValue({ kind, message })`。

### Pinia

```ts
import { exposePinia } from '@app-mcp/store/pinia'

const cart = useCartStore()   // 需在 pinia 激活后调用

exposePinia(appMcp, cart, {
  namespace: 'cart',
  actions: {
    add: { description: '加入购物车', input: itemSchema },          // cart.add(input)
    checkout: {
      description: '结算',
      risk: 'payment',
      enabled: (s) => s.items.length > 0,                         // s 为 cart.$state
      result: (_s, order) => order,
    },
  },
  resources: {
    items: { description: '购物车商品', select: (s) => s.items },
  },
})
```

- state 取 `store.$state`，订阅用 `store.$subscribe(..., { detached: true, flush: 'sync' })`——组件卸载不会取消订阅，需自行调用 `dispose()`。
- Pinia 的 state 是原地修改的，选中的数组 / 对象引用可能不变而内容已变，因此资源变化检测改为比较 **JSON 快照**；
  自定义 `equals` 收到的也是前后两次的快照副本。

### 通用核心

```ts
import { exposeStore } from '@app-mcp/store'

exposeStore(appMcp, { getState: () => state, subscribe: (l) => emitter.on('change', l) }, { actions, resources })
```

适配器还可以提供可选的 `invoke(call)`（自定义执行方式，缺省在 `getState()` 上按名称调用函数）与 `mutable: true`（原地修改的 state）。

## 选项

| 字段 | 说明 |
|---|---|
| `namespace` | 工具名与资源名前缀：`'cart'` → `cart.add`、`cart.items` |
| `actions[name].description / title / input / risk / activation` | 原样传给 `ToolDefinition` |
| `actions[name].action` | Zustand / Pinia：函数名；Redux：无 `creator` 时的 action type。缺省为工具名最后一段 |
| `actions[name].creator` | Redux：action creator / thunk / createAsyncThunk |
| `actions[name].args` | 参数对象 → 位置参数。缺省：声明了 `input` 时 `[input]`，否则 `[]` |
| `actions[name].enabled` | `(state) => boolean`，状态变化时重算，只有结果变化才 `update({ enabled })` |
| `actions[name].result` | `(state, returned) => 结果`，state 为 action 执行后的最新状态 |
| `actions[name].hints` | 结果附带的 stateHints |
| `resources[name].select` | 从 state 选出资源内容 |
| `resources[name].equals` | 自定义相等判断；缺省浅比较（Pinia 为 JSON 快照比较） |

返回 `dispose()`：注销全部工具与资源、取消订阅，可重复调用。

## 行为说明

- `enabled` 重算与资源变化检测共用一个 store 订阅；同一微任务内的多次变化只处理一次。
- 调用时会再次检查 `enabled(state)`，为 false 则抛出 `TOOL_DISABLED`（防止 Host 尚未收到禁用通知时的竞态）。
  `enabled` 自身抛出异常时视为不可用。
- action 抛出的 `ToolCallError` 原样透传；其他异常由 SDK 归为 `HANDLER_ERROR`。
- 默认结果为 action 的返回值（`undefined` 时为 `{ ok: true }`）。**结果必须可 JSON 序列化**：若 action 返回函数、
  类实例、循环引用等（例如 Zustand 的 action 返回一个 unsubscribe 函数，或 Pinia 返回响应式对象），
  请用 `result` 选项挑出需要的数据，否则 SDK 会以 `HANDLER_ERROR` 报告序列化失败或丢失字段。
- 注册名称重复等错误会在 `expose*` 时同步抛出，已注册的部分会被回滚；action 名在 state / store 上不存在时同样立即报错。
