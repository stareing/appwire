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
