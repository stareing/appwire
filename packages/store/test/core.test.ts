import { isToolResultEnvelope } from '@app-mcp/web'
import { describe, expect, it } from 'vitest'
import { exposeStore, shallowEqual, type StoreAdapter } from '../src/index'
import { FakeRegistrar, flush } from './fake'

/** 最小手写 store，统计订阅与 getState 次数。 */
function makeAdapter<S extends object>(initial: S) {
  let state = initial
  const listeners = new Set<() => void>()
  const stats = { subscribes: 0, unsubscribes: 0 }
  const adapter: StoreAdapter<S> = {
    getState: () => state,
    subscribe: (l) => {
      stats.subscribes++
      listeners.add(l)
      return () => {
        stats.unsubscribes++
        listeners.delete(l)
      }
    },
  }
  const set = (patch: Partial<S>) => {
    state = { ...state, ...patch }
    for (const l of listeners) l()
  }
  return { adapter, set, stats, listeners }
}

describe('exposeStore', () => {
  it('只订阅一次；高频变化在一个微任务内只处理一次', async () => {
    const reg = new FakeRegistrar()
    let selects = 0
    const { adapter, set, stats } = makeAdapter({ n: 0, inc: () => {} })
    const dispose = exposeStore(reg, adapter, {
      actions: { inc: { description: '加一', enabled: (s) => s.n < 100 } },
      resources: {
        n: {
          description: 'n',
          select: (s) => {
            selects++
            return s.n
          },
        },
      },
    })
    expect(stats.subscribes).toBe(1)
    const before = selects
    for (let i = 1; i <= 50; i++) set({ n: i })
    await flush()
    expect(selects - before).toBe(1)
    expect(reg.getResource('n').notifies).toBe(1)
    set({ n: 200 })
    await flush()
    expect(reg.getTool('inc').updates).toEqual([{ enabled: false }])
    dispose()
    expect(stats.unsubscribes).toBe(1)
  })

  it('annotations 与 outputSchema 原样交给 SDK；未给出时不声明', () => {
    const reg = new FakeRegistrar()
    const { adapter } = makeAdapter({ cancel: () => {}, go: () => {} })
    exposeStore(reg, adapter, {
      actions: {
        cancel: {
          description: '取消',
          risk: 'destructive',
          annotations: { idempotentHint: true },
          outputSchema: { type: 'object', properties: { ok: { type: 'boolean' } } },
        },
        go: { description: 'x' },
      },
    })
    expect(reg.getTool('cancel').def).toMatchObject({
      risk: 'destructive',
      annotations: { idempotentHint: true },
      outputSchema: { type: 'object', properties: { ok: { type: 'boolean' } } },
    })
    expect(reg.getTool('go').def).not.toHaveProperty('annotations')
    expect(reg.getTool('go').def).not.toHaveProperty('outputSchema')
  })

  it('没有 enabled 与资源时不订阅', () => {
    const reg = new FakeRegistrar()
    const { adapter, stats } = makeAdapter({ go: () => 1 })
    exposeStore(reg, adapter, { actions: { go: { description: 'x' } } })
    expect(stats.subscribes).toBe(0)
  })

  it('自定义 invoke 收到解析后的调用信息', async () => {
    const reg = new FakeRegistrar()
    const calls: unknown[] = []
    exposeStore(
      reg,
      {
        getState: () => ({}),
        subscribe: () => () => {},
        invoke: (c) => {
          calls.push({ tool: c.tool, action: c.action, args: c.args, input: c.input })
          return Promise.resolve(7)
        },
      },
      { namespace: 'ns', actions: { 'a.b': { description: 'x', input: { type: 'object' } } } },
    )
    expect(await reg.call('ns.a.b', { k: 1 })).toEqual({ data: 7 })
    expect(calls).toEqual([{ tool: 'ns.a.b', action: 'b', args: [{ k: 1 }], input: { k: 1 } }])
  })

  it('enabled 抛出异常时视为不可用', async () => {
    const reg = new FakeRegistrar()
    const { adapter } = makeAdapter({ go: () => 1 })
    exposeStore(reg, adapter, {
      actions: {
        go: {
          description: 'x',
          enabled: () => {
            throw new Error('x')
          },
        },
      },
    })
    expect(reg.getTool('go').def.enabled).toBe(false)
    await expect(reg.call('go')).rejects.toMatchObject({ kind: 'TOOL_DISABLED' })
  })

  it('shallowEqual', () => {
    expect(shallowEqual(1, 1)).toBe(true)
    expect(shallowEqual(NaN, NaN)).toBe(true)
    expect(shallowEqual({ a: 1 }, { a: 1 })).toBe(true)
    expect(shallowEqual({ a: 1 }, { a: 1, b: 2 })).toBe(false)
    expect(shallowEqual({ a: {} }, { a: {} })).toBe(false)
    expect(shallowEqual([1, 2], [1, 2])).toBe(true)
    expect(shallowEqual([1, 2], { 0: 1, 1: 2 })).toBe(false)
    expect(shallowEqual(null, {})).toBe(false)
  })
})

describe('resultEnvelope', () => {
  const actions = {
    done: () => undefined,
    ship: () => ({ data: { id: 'o1' }, status: 'pending' as const, stateResource: 'order.state', stateHints: ['order.list'] }),
    plain: () => ({ data: 1, other: 2 }),
    bare: () => ({ data: 1 }),
    partial: () => ({ data: null, status: 'partial' as const, summary: '完成 2/3' }),
  }

  it('默认关闭：保持 { data } 包装与 { ok: true }', async () => {
    const reg = new FakeRegistrar()
    const { adapter } = makeAdapter(actions)
    exposeStore(reg, adapter, { actions: { done: { description: 'd' }, bare: { description: 'b' } } })
    expect(await reg.call('done')).toEqual({ data: { ok: true } })
    expect(await reg.call('bare')).toEqual({ data: { data: 1 } })
  })

  it('开启：无返回值为 { data: undefined }（Hub 输出"已完成"），不再是 { ok: true }', async () => {
    const reg = new FakeRegistrar()
    const { adapter } = makeAdapter(actions)
    exposeStore(reg, adapter, { resultEnvelope: true, actions: { done: { description: 'd' } } })
    const res = await reg.call('done')
    expect(res).toEqual({ data: undefined })
    expect(isToolResultEnvelope(res)).toBe(true)
  })

  it('开启：信封原样透传，hints 去重并入 stateHints', async () => {
    const reg = new FakeRegistrar()
    const { adapter } = makeAdapter(actions)
    exposeStore(reg, adapter, {
      resultEnvelope: true,
      actions: {
        ship: { description: 's', hints: ['order.list', 'cart.items'] },
        partial: { description: 'p' },
      },
    })
    expect(await reg.call('ship')).toEqual({
      data: { id: 'o1' },
      status: 'pending',
      stateResource: 'order.state',
      stateHints: ['order.list', 'cart.items'],
    })
    expect(await reg.call('partial')).toEqual({ data: null, status: 'partial', summary: '完成 2/3' })
  })

  it('开启：非信封值作为 data，附带 hints', async () => {
    const reg = new FakeRegistrar()
    const { adapter } = makeAdapter(actions)
    exposeStore(reg, adapter, { resultEnvelope: true, actions: { plain: { description: 'p', hints: ['x'] } } })
    expect(await reg.call('plain')).toEqual({ data: { data: 1, other: 2 }, stateHints: ['x'] })
  })

  it('result 选择器可返回信封；单个 action 的 resultEnvelope 覆盖全局', async () => {
    const reg = new FakeRegistrar()
    const { adapter } = makeAdapter({ ...actions, n: 3 })
    exposeStore(reg, adapter, {
      actions: {
        done: {
          description: 'd',
          resultEnvelope: true,
          result: (st) => ({ data: st.n, summary: `剩余 ${st.n} 件` }),
        },
      },
    })
    expect(await reg.call('done')).toEqual({ data: 3, summary: '剩余 3 件' })

    const reg2 = new FakeRegistrar()
    exposeStore(reg2, makeAdapter(actions).adapter, {
      resultEnvelope: true,
      actions: { done: { description: 'd', resultEnvelope: false } },
    })
    expect(await reg2.call('done')).toEqual({ data: { ok: true } })
  })
})
