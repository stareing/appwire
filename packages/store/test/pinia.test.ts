import { ToolCallError } from '@app-mcp/web'
import { createPinia, defineStore, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { exposePinia } from '../src/pinia'
import { FakeRegistrar, flush } from './fake'

interface Item {
  id: string
  qty: number
}

const useCart = defineStore('cart', {
  state: () => ({ items: [] as Item[], locked: false }),
  getters: {
    count: (s) => s.items.length,
  },
  actions: {
    add(item: Item) {
      this.items.push({ ...item })
      return this.items.length
    },
    setQty(id: string, qty: number) {
      const it = this.items.find((i) => i.id === id)
      if (!it) throw new ToolCallError('INVALID_INPUT', `没有商品 ${id}`)
      it.qty = qty
    },
    async checkout() {
      await Promise.resolve()
      if (this.items.length === 0) throw new Error('购物车为空')
      const n = this.items.length
      this.items = []
      return { orderId: 'o1', n }
    },
    lock(locked: boolean) {
      this.locked = locked
    },
  },
})

const itemSchema = { type: 'object', properties: { id: { type: 'string' }, qty: { type: 'number' } } } as const

describe('exposePinia', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
  })

  it('注册名称、调用 action、参数映射与结果', async () => {
    const reg = new FakeRegistrar()
    const cart = useCart()
    exposePinia(reg, cart, {
      namespace: 'cart',
      actions: {
        add: { description: '加入', input: itemSchema },
        'item.qty': {
          description: '改数量',
          input: itemSchema,
          action: 'setQty',
          args: (i: Item) => [i.id, i.qty],
          result: (s) => s.items,
          hints: ['cart.items'],
        },
        checkout: { description: '结算', risk: 'payment' },
      },
    })
    expect([...reg.tools.keys()]).toEqual(['cart.add', 'cart.item.qty', 'cart.checkout'])
    expect(await reg.call('cart.add', { id: 'a', qty: 1 })).toEqual({ data: 1 })
    expect(await reg.call('cart.item.qty', { id: 'a', qty: 4 })).toEqual({
      data: [{ id: 'a', qty: 4 }],
      stateHints: ['cart.items'],
    })
    await expect(reg.call('cart.item.qty', { id: 'zz', qty: 1 })).rejects.toBeInstanceOf(ToolCallError)
    expect(await reg.call('cart.checkout')).toEqual({ data: { orderId: 'o1', n: 1 } })
    const err = await reg.call('cart.checkout').catch((e: unknown) => e)
    expect(err).not.toBeInstanceOf(ToolCallError)
    expect((err as Error).message).toBe('购物车为空')
  })

  it('不存在的 action 在 expose 时报错', () => {
    const reg = new FakeRegistrar()
    expect(() => exposePinia(reg, useCart(), { actions: { nope: { description: 'x' } } })).toThrow(/nope/)
    // state 字段不是函数
    expect(() => exposePinia(reg, useCart(), { actions: { items: { description: 'x' } } })).toThrow(/items/)
  })

  it('原地修改也能检测资源变化（JSON 快照），且只在内容变化时 notify', async () => {
    const reg = new FakeRegistrar()
    const cart = useCart()
    exposePinia(reg, cart, {
      actions: {},
      resources: {
        items: { description: '商品', select: (s) => s.items },
        locked: { description: '锁定', select: (s) => s.locked },
      },
    })
    const items = reg.getResource('items')
    const locked = reg.getResource('locked')

    cart.add({ id: 'a', qty: 1 })
    cart.add({ id: 'b', qty: 1 })
    await flush()
    expect([items.notifies, locked.notifies]).toEqual([1, 0])

    cart.setQty('a', 9) // 数组引用不变，内容变化
    await flush()
    expect(items.notifies).toBe(2)

    cart.setQty('a', 9) // 内容不变
    cart.lock(true)
    await flush()
    expect([items.notifies, locked.notifies]).toEqual([2, 1])

    cart.$patch({ locked: false })
    await flush()
    expect(locked.notifies).toBe(2)
    expect(await items.def.read()).toEqual([
      { id: 'a', qty: 9 },
      { id: 'b', qty: 1 },
    ])
  })

  it('自定义 equals 拿到的是快照，不受原地修改影响', async () => {
    const reg = new FakeRegistrar()
    const cart = useCart()
    const equals = vi.fn((a: unknown, b: unknown) => (a as Item[]).length === (b as Item[]).length)
    exposePinia(reg, cart, { actions: {}, resources: { items: { description: '商品', select: (s) => s.items, equals } } })
    cart.add({ id: 'a', qty: 1 })
    await flush()
    expect(equals).toHaveBeenLastCalledWith([], [{ id: 'a', qty: 1 }])
    expect(reg.getResource('items').notifies).toBe(1)
  })

  it('enabled 随状态变化 update；禁用时调用返回 TOOL_DISABLED', async () => {
    const reg = new FakeRegistrar()
    const cart = useCart()
    exposePinia(reg, cart, {
      actions: {
        checkout: { description: '结算', enabled: (s) => s.items.length > 0 && !s.locked },
      },
    })
    const tool = reg.getTool('checkout')
    expect(tool.def.enabled).toBe(false)
    await expect(reg.call('checkout')).rejects.toMatchObject({ kind: 'TOOL_DISABLED' })

    cart.add({ id: 'a', qty: 1 })
    cart.add({ id: 'b', qty: 1 })
    await flush()
    expect(tool.updates).toEqual([{ enabled: true }])
    cart.setQty('a', 2)
    await flush()
    expect(tool.updates).toHaveLength(1)
    cart.lock(true)
    await flush()
    expect(tool.updates).toEqual([{ enabled: true }, { enabled: false }])
  })

  it('dispose 注销并取消 $subscribe', async () => {
    const reg = new FakeRegistrar()
    const cart = useCart()
    const sub = vi.spyOn(cart, '$subscribe')
    const dispose = exposePinia(reg, cart, {
      actions: { add: { description: '加入', input: itemSchema } },
      resources: { items: { description: '商品', select: (s) => s.items } },
    })
    expect(sub).toHaveBeenCalledTimes(1)
    expect(sub.mock.calls[0]![1]).toMatchObject({ detached: true })
    const res = reg.getResource('items')
    dispose()
    cart.add({ id: 'a', qty: 1 })
    await flush()
    expect(res.notifies).toBe(0)
    expect(reg.tools.size + reg.resources.size).toBe(0)
  })
})
