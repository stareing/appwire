import { configureStore, createAsyncThunk, createSlice, type PayloadAction } from '@reduxjs/toolkit'
import { ToolCallError } from '@app-mcp/web'
import { describe, expect, it, vi } from 'vitest'
import { exposeRedux, toCallError } from '../src/redux'
import { FakeRegistrar, flush } from './fake'

interface Todo {
  id: number
  text: string
  done: boolean
}
interface TodosState {
  items: Todo[]
  loading: boolean
}

const fetchTodos = createAsyncThunk('todos/fetch', async (count: number) =>
  Array.from({ length: count }, (_, i) => ({ id: 100 + i, text: `远程 ${i}`, done: false })),
)
const failPlain = createAsyncThunk('todos/failPlain', async () => {
  throw new Error('网络错误')
})
const failWithKind = createAsyncThunk('todos/failWithKind', async (_: void, { rejectWithValue }) =>
  rejectWithValue({ kind: 'INVALID_INPUT', message: '标题太长', details: { max: 10 } }),
)
const failWithCode = createAsyncThunk('todos/failWithCode', async () => {
  throw Object.assign(new Error('没登录'), { code: 'UNAUTHORIZED' })
})

const slice = createSlice({
  name: 'todos',
  initialState: { items: [], loading: false } as TodosState,
  reducers: {
    add: {
      reducer: (s, a: PayloadAction<Todo>) => {
        s.items.push(a.payload)
      },
      prepare: (text: string, id: number) => ({ payload: { id, text, done: false } }),
    },
    toggle: (s, a: PayloadAction<{ id: number }>) => {
      const t = s.items.find((i) => i.id === a.payload.id)
      if (t) t.done = !t.done
    },
    clear: (s) => {
      s.items = []
    },
  },
  extraReducers: (b) => {
    b.addCase(fetchTodos.pending, (s) => {
      s.loading = true
    })
    b.addCase(fetchTodos.fulfilled, (s, a) => {
      s.loading = false
      s.items.push(...a.payload)
    })
  },
})

function makeStore() {
  return configureStore({ reducer: { todos: slice.reducer } })
}
type Root = ReturnType<ReturnType<typeof makeStore>['getState']>

const addSchema = { type: 'object', properties: { text: { type: 'string' }, id: { type: 'number' } } } as const
const idSchema = { type: 'object', properties: { id: { type: 'number' } } } as const

describe('exposeRedux', () => {
  it('普通 action creator：dispatch(creator(input))，默认结果 { ok: true }', async () => {
    const reg = new FakeRegistrar()
    const store = makeStore()
    exposeRedux(reg, store, {
      namespace: 'todos',
      actions: {
        add: {
          description: '新增',
          input: addSchema,
          creator: slice.actions.add,
          args: (i: { text: string; id: number }) => [i.text, i.id],
        },
        toggle: { description: '切换', input: idSchema, creator: slice.actions.toggle },
        clear: { description: '清空', creator: slice.actions.clear, risk: 'destructive' },
      },
    })
    expect([...reg.tools.keys()]).toEqual(['todos.add', 'todos.toggle', 'todos.clear'])
    expect(await reg.call('todos.add', { text: '买菜', id: 1 })).toEqual({ data: { ok: true } })
    expect(await reg.call('todos.toggle', { id: 1 })).toEqual({ data: { ok: true } })
    expect(store.getState().todos.items).toEqual([{ id: 1, text: '买菜', done: true }])
    await reg.call('todos.clear')
    expect(store.getState().todos.items).toEqual([])
  })

  it('没有 creator 时用 action 作为 type 分发 { type, payload }', async () => {
    const reg = new FakeRegistrar()
    const store = makeStore()
    exposeRedux(reg, store, {
      actions: { toggle: { description: '切换', input: idSchema, action: 'todos/toggle' } },
    })
    store.dispatch(slice.actions.add('a', 1))
    await reg.call('toggle', { id: 1 })
    expect(store.getState().todos.items[0]!.done).toBe(true)
  })

  it('既没有 creator 也没有 action 时报错', () => {
    const reg = new FakeRegistrar()
    expect(() => exposeRedux(reg, makeStore(), { actions: { x: { description: 'x' } } })).toThrow(/creator/)
    expect(reg.tools.size).toBe(0)
  })

  it('结果选择器与 hints', async () => {
    const reg = new FakeRegistrar()
    const store = makeStore()
    exposeRedux(reg, store, {
      actions: {
        add: {
          description: '新增',
          input: addSchema,
          creator: (i: { text: string; id: number }) => slice.actions.add(i.text, i.id),
          result: (s: Root) => ({ count: s.todos.items.length }),
          hints: ['todos.list'],
        },
      },
    })
    expect(await reg.call('add', { text: 'a', id: 1 })).toEqual({ data: { count: 1 }, stateHints: ['todos.list'] })
  })

  it('普通 thunk：同步与返回 Promise 的返回值', async () => {
    const reg = new FakeRegistrar()
    const store = makeStore()
    const syncThunk = (text: string) => (dispatch: (a: unknown) => unknown, getState: () => Root) => {
      dispatch(slice.actions.add(text, getState().todos.items.length + 1))
      return getState().todos.items.length
    }
    const asyncThunk = () => async (dispatch: (a: unknown) => unknown) => {
      await Promise.resolve()
      dispatch(slice.actions.clear())
      return 'cleared'
    }
    const failingThunk = () => async () => {
      throw new ToolCallError('USER_REJECTED', '用户取消')
    }
    exposeRedux(reg, store, {
      actions: {
        addSync: { description: 'a', input: { type: 'object' }, creator: (i: { text: string }) => syncThunk(i.text) },
        clearAsync: { description: 'b', creator: asyncThunk },
        failing: { description: 'c', creator: failingThunk },
      },
    })
    expect(await reg.call('addSync', { text: 'x' })).toEqual({ data: 1 })
    expect(await reg.call('clearAsync')).toEqual({ data: 'cleared' })
    expect(store.getState().todos.items).toEqual([])
    await expect(reg.call('failing')).rejects.toMatchObject({ kind: 'USER_REJECTED' })
  })

  it('createAsyncThunk：fulfilled 返回 payload（unwrap 语义）', async () => {
    const reg = new FakeRegistrar()
    const store = makeStore()
    exposeRedux(reg, store, {
      actions: {
        fetch: {
          description: '拉取',
          input: { type: 'object', properties: { count: { type: 'number' } } },
          creator: (i: { count: number }) => fetchTodos(i.count),
        },
      },
    })
    const r = await reg.call('fetch', { count: 2 })
    expect(r.data).toHaveLength(2)
    expect(r.data[0]).toMatchObject({ id: 100 })
    expect(store.getState().todos.items).toHaveLength(2)
  })

  it('createAsyncThunk：rejected 转换为 HANDLER_ERROR 或 ToolCallError', async () => {
    const reg = new FakeRegistrar()
    const store = makeStore()
    exposeRedux(reg, store, {
      actions: {
        plain: { description: 'a', creator: failPlain },
        kind: { description: 'b', creator: failWithKind },
        code: { description: 'c', creator: failWithCode },
      },
    })
    const e1 = await reg.call('plain').catch((e: unknown) => e)
    expect(e1).toBeInstanceOf(Error)
    expect(e1).not.toBeInstanceOf(ToolCallError)
    expect((e1 as Error).message).toBe('网络错误')

    const e2 = await reg.call('kind').catch((e: unknown) => e)
    expect(e2).toBeInstanceOf(ToolCallError)
    expect(e2).toMatchObject({ kind: 'INVALID_INPUT', message: '标题太长', details: { max: 10 } })

    const e3 = await reg.call('code').catch((e: unknown) => e)
    expect(e3).toBeInstanceOf(ToolCallError)
    expect(e3).toMatchObject({ kind: 'UNAUTHORIZED', message: '没登录' })
  })

  it('toCallError：各种 rejected 原因', () => {
    const tce = new ToolCallError('TIMEOUT', 't')
    expect(toCallError(tce)).toBe(tce)
    expect(toCallError({ kind: 'NOT_A_KIND', message: 'm' })).not.toBeInstanceOf(ToolCallError)
    expect(toCallError({ kind: 'RATE_LIMITED', message: 'm' })).toMatchObject({ kind: 'RATE_LIMITED' })
    expect(toCallError({ code: 'PAYLOAD_TOO_LARGE', message: 'm' })).toMatchObject({ kind: 'PAYLOAD_TOO_LARGE' })
    expect(toCallError('坏了').message).toContain('坏了')
    expect(toCallError(42).message).toContain('42')
  })

  it('enabled 与资源：一次订阅、微任务合并、变化才 update / notify', async () => {
    const reg = new FakeRegistrar()
    const store = makeStore()
    const subscribe = vi.spyOn(store, 'subscribe')
    const dispose = exposeRedux(reg, store, {
      namespace: 'todos',
      actions: {
        clear: {
          description: '清空',
          creator: slice.actions.clear,
          enabled: (s: Root) => s.todos.items.length > 0,
        },
        toggle: {
          description: '切换',
          input: idSchema,
          creator: slice.actions.toggle,
          enabled: (s: Root) => s.todos.items.length > 0,
        },
      },
      resources: {
        list: { description: '列表', select: (s: Root) => s.todos.items },
        loading: { description: '加载中', select: (s: Root) => s.todos.loading },
      },
    })
    expect(subscribe).toHaveBeenCalledTimes(1)
    const clear = reg.getTool('todos.clear')
    const list = reg.getResource('todos.list')
    const loading = reg.getResource('todos.loading')
    expect(clear.def.enabled).toBe(false)

    await expect(reg.call('todos.clear')).rejects.toMatchObject({ kind: 'TOOL_DISABLED' })

    store.dispatch(slice.actions.add('a', 1))
    store.dispatch(slice.actions.add('b', 2))
    await flush()
    expect(clear.updates).toEqual([{ enabled: true }])
    expect(reg.getTool('todos.toggle').updates).toEqual([{ enabled: true }])
    expect([list.notifies, loading.notifies]).toEqual([1, 0])

    store.dispatch({ type: 'unrelated' })
    await flush()
    expect([list.notifies, loading.notifies]).toEqual([1, 0])
    expect(clear.updates).toHaveLength(1)

    await reg.call('todos.clear')
    await flush()
    expect(clear.updates).toEqual([{ enabled: true }, { enabled: false }])
    expect(list.notifies).toBe(2)
    expect(await list.def.read()).toEqual([])

    const unsubscribe = subscribe.mock.results[0]!.value as () => void
    expect(typeof unsubscribe).toBe('function')
    dispose()
    store.dispatch(slice.actions.add('c', 3))
    await flush()
    expect(list.notifies).toBe(2)
    expect(reg.tools.size + reg.resources.size).toBe(0)
  })
})
