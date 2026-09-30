import { ToolCallError } from '@app-mcp/web'
import { describe, expect, it } from 'vitest'
import { z } from 'zod'
import { createStore } from 'zustand/vanilla'
import { exposeZustand } from '../src/zustand'
import { FakeRegistrar, flush } from './fake'

interface Item {
  id: string
  qty: number
}
interface CartState {
  items: Item[]
  locked: boolean
  note: string
  add(item: Item): number
  setQty(id: string, qty: number): void
  clear(): void
  lock(locked: boolean): void
  setNote(note: string): void
  fail(): void
  failKind(): void
  makeFn(): () => void
}

function makeStore() {
  return createStore<CartState>()((set, get) => ({
    items: [],
    locked: false,
    note: '',
    add: (item) => {
      set({ items: [...get().items, item] })
      return get().items.length
    },
    setQty: (id, qty) => set({ items: get().items.map((i) => (i.id === id ? { ...i, qty } : i)) }),
    clear: () => set({ items: [] }),
    lock: (locked) => set({ locked }),
    setNote: (note) => set({ note }),
    fail: () => {
      throw new Error('炸了')
    },
    failKind: () => {
      throw new ToolCallError('INVALID_INPUT', '数量不对', { field: 'qty' })
    },
    makeFn: () => () => {},
  }))
}

const itemSchema = { type: 'object', properties: { id: { type: 'string' }, qty: { type: 'number' } } } as const

describe('exposeZustand', () => {
  it('按 namespace 注册工具与资源，定义字段透传', () => {
    const reg = new FakeRegistrar()
    const store = makeStore()
    const zodInput = z.object({ note: z.string() })
    exposeZustand(reg, store, {
      namespace: 'cart',
      actions: {
        add: { description: '加入', title: '加入购物车', input: itemSchema, risk: 'write', activation: 'background' },
        clear: { description: '清空', risk: 'destructive' },
        'note.set': { description: '备注', input: zodInput, action: 'setNote' },
      },
      resources: { items: { description: '购物车', select: (s) => s.items } },
    })
    expect([...reg.tools.keys()]).toEqual(['cart.add', 'cart.clear', 'cart.note.set'])
    expect([...reg.resources.keys()]).toEqual(['cart.items'])
    const add = reg.getTool('cart.add').def
    expect(add).toMatchObject({ description: '加入', title: '加入购物车', input: itemSchema, risk: 'write', activation: 'background' })
    expect('enabled' in add).toBe(false)
    expect(reg.getTool('cart.note.set').def.input).toBe(zodInput)
    expect(reg.getTool('cart.clear').def.input).toBeUndefined()
  })

  it('无 namespace 时使用原名；action 名缺省取最后一段', async () => {
    const reg = new FakeRegistrar()
    const store = makeStore()
    exposeZustand(reg, store, { actions: { 'x.clear': { description: '清空' } } })
    store.getState().add({ id: 'a', qty: 1 })
    await reg.call('x.clear')
    expect(store.getState().items).toEqual([])
  })

  it('参数映射：有 input 时传 [input]，无 input 时 []，args 自定义位置参数', async () => {
    const reg = new FakeRegistrar()
    const store = makeStore()
    exposeZustand(reg, store, {
      actions: {
        add: { description: '加入', input: itemSchema },
        setQty: {
          description: '改数量',
          input: itemSchema,
          args: (i: Item) => [i.id, i.qty],
        },
        clear: { description: '清空' },
      },
    })
    expect(await reg.call('add', { id: 'a', qty: 1 })).toEqual({ data: 1 })
    expect(await reg.call('setQty', { id: 'a', qty: 5 })).toEqual({ data: { ok: true } })
    expect(store.getState().items).toEqual([{ id: 'a', qty: 5 }])
    expect(await reg.call('clear', { ignored: true })).toEqual({ data: { ok: true } })
    expect(store.getState().items).toEqual([])
  })

  it('结果选择器使用 action 之后的 state，并附带 hints', async () => {
    const reg = new FakeRegistrar()
    const store = makeStore()
    exposeZustand(reg, store, {
      actions: {
        add: {
          description: '加入',
          input: itemSchema,
          result: (s, returned) => ({ count: s.items.length, returned }),
          hints: ['cart.items'],
        },
      },
    })
    expect(await reg.call('add', { id: 'a', qty: 2 })).toEqual({
      data: { count: 1, returned: 1 },
      stateHints: ['cart.items'],
    })
  })

  it('返回值为 { data } 形状时仍被包装，不会被误拆', async () => {
    const reg = new FakeRegistrar()
    const store = createStore<{ get(): { data: number } }>()(() => ({ get: () => ({ data: 1 }) }))
    exposeZustand(reg, store, { actions: { get: { description: '取' } } })
    expect(await reg.call('get')).toEqual({ data: { data: 1 } })
  })

  it('enabled 随状态变化 update，且同一微任务内合并、值不变不重复 update', async () => {
    const reg = new FakeRegistrar()
    const store = makeStore()
    exposeZustand(reg, store, {
      actions: { clear: { description: '清空', enabled: (s) => s.items.length > 0 && !s.locked } },
    })
    const tool = reg.getTool('clear')
    expect(tool.def.enabled).toBe(false)

    store.getState().add({ id: 'a', qty: 1 })
    store.getState().add({ id: 'b', qty: 1 })
    store.getState().setNote('x')
    await flush()
    expect(tool.updates).toEqual([{ enabled: true }])

    store.getState().add({ id: 'c', qty: 1 })
    store.getState().setNote('y')
    await flush()
    expect(tool.updates).toHaveLength(1)

    // 同一微任务内变为 false 又恢复 true：最终值不变，不 update
    store.getState().lock(true)
    store.getState().lock(false)
    await flush()
    expect(tool.updates).toHaveLength(1)

    store.getState().lock(true)
    await flush()
    expect(tool.updates).toEqual([{ enabled: true }, { enabled: false }])
    // 只传 enabled 字段
    expect(Object.keys(tool.updates[1]!)).toEqual(['enabled'])
  })

  it('禁用时调用返回 TOOL_DISABLED（竞态保护），且不执行 action', async () => {
    const reg = new FakeRegistrar()
    const store = makeStore()
    exposeZustand(reg, store, {
      actions: { add: { description: '加入', input: itemSchema, enabled: (s) => !s.locked } },
    })
    store.getState().lock(true) // 尚未处理订阅，工具在 SDK 看来仍可用
    const err = await reg.call('add', { id: 'a', qty: 1 }).catch((e: unknown) => e)
    expect(err).toBeInstanceOf(ToolCallError)
    expect((err as ToolCallError).kind).toBe('TOOL_DISABLED')
    expect(store.getState().items).toEqual([])
  })

  it('action 抛出 ToolCallError 原样透传，其他异常原样抛给 SDK', async () => {
    const reg = new FakeRegistrar()
    const store = makeStore()
    exposeZustand(reg, store, { actions: { fail: { description: 'x' }, failKind: { description: 'y' } } })
    const e1 = await reg.call('failKind').catch((e: unknown) => e)
    expect(e1).toBeInstanceOf(ToolCallError)
    expect(e1).toMatchObject({ kind: 'INVALID_INPUT', message: '数量不对', details: { field: 'qty' } })
    const e2 = await reg.call('fail').catch((e: unknown) => e)
    expect(e2).not.toBeInstanceOf(ToolCallError)
    expect((e2 as Error).message).toBe('炸了')
  })

  it('不可序列化的返回值原样交给 SDK', async () => {
    const reg = new FakeRegistrar()
    const store = makeStore()
    exposeZustand(reg, store, { actions: { makeFn: { description: 'x' } } })
    const r = await reg.call('makeFn')
    expect(typeof r.data).toBe('function')
  })

  it('资源：select 结果变化才 notify（默认浅比较），read 返回当前值', async () => {
    const reg = new FakeRegistrar()
    const store = makeStore()
    exposeZustand(reg, store, {
      actions: {},
      resources: {
        items: { description: '商品', select: (s) => s.items },
        summary: { description: '摘要', select: (s) => ({ count: s.items.length, locked: s.locked }) },
        noteLen: {
          description: '备注长度（自定义 equals）',
          select: (s) => s.note,
          equals: (a, b) => (a as string).length === (b as string).length,
        },
      },
    })
    const items = reg.getResource('items')
    const summary = reg.getResource('summary')
    const note = reg.getResource('noteLen')

    store.getState().setNote('ab')
    await flush()
    expect([items.notifies, summary.notifies, note.notifies]).toEqual([0, 0, 1])

    store.getState().setNote('cd') // 长度相同
    await flush()
    expect(note.notifies).toBe(1)

    store.getState().add({ id: 'a', qty: 1 })
    store.getState().add({ id: 'b', qty: 1 })
    await flush()
    expect([items.notifies, summary.notifies]).toEqual([1, 1])
    expect(await items.def.read()).toEqual([
      { id: 'a', qty: 1 },
      { id: 'b', qty: 1 },
    ])

    store.getState().setQty('a', 3) // 数量不变 → summary 浅比较相同
    await flush()
    expect([items.notifies, summary.notifies]).toEqual([2, 1])
  })

  it('dispose 注销全部工具与资源并取消订阅', async () => {
    const reg = new FakeRegistrar()
    const store = makeStore()
    const dispose = exposeZustand(reg, store, {
      namespace: 'cart',
      actions: { clear: { description: '清空', enabled: (s) => s.items.length > 0 } },
      resources: { items: { description: '商品', select: (s) => s.items } },
    })
    const tool = reg.getTool('cart.clear')
    const res = reg.getResource('cart.items')
    store.getState().add({ id: 'a', qty: 1 }) // 已排队的微任务也不应在 dispose 后生效
    dispose()
    await flush()
    expect(reg.tools.size).toBe(0)
    expect(reg.resources.size).toBe(0)
    expect(tool.disposed && res.disposed).toBe(true)
    expect(tool.updates).toEqual([])
    expect(res.notifies).toBe(0)
    // 取消订阅：后续变化不再触发
    store.getState().add({ id: 'b', qty: 1 })
    await flush()
    expect(res.notifies).toBe(0)
    dispose() // 可重复调用
  })

  it('注册失败时回滚已注册项；不存在的 action 在 expose 时报错', () => {
    const reg = new FakeRegistrar()
    const store = makeStore()
    reg.tool('clear', { description: '占位', handler: () => null })
    expect(() =>
      exposeZustand(reg, store, { actions: { add: { description: 'a' }, clear: { description: 'b' } } }),
    ).toThrow(/已注册/)
    expect([...reg.tools.keys()]).toEqual(['clear'])
    expect(() => exposeZustand(reg, store, { actions: { nope: { description: 'x' } } })).toThrow(/nope/)
  })
})
