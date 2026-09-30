import { Profiler, StrictMode, useState, type ReactNode } from 'react'
import { act, cleanup, render } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import { z } from 'zod'
import type { ToolHandle } from '@app-mcp/web'
import {
  AppMcpProvider,
  ToolScope,
  useAppMcp,
  useConnectionState,
  useHold,
  useResource,
  useTool,
} from './index'
import { FakeAppMcp } from './testing/fake-app-mcp'

afterEach(() => cleanup())

function setup(ui: ReactNode, options: { strict?: boolean } = {}) {
  const app = new FakeAppMcp()
  const wrap = (node: ReactNode) => {
    const tree = <AppMcpProvider value={app}>{node}</AppMcpProvider>
    return options.strict ? <StrictMode>{tree}</StrictMode> : tree
  }
  const result = render(wrap(ui))
  return { app, ...result, rerender: (node: ReactNode) => result.rerender(wrap(node)) }
}

function Counter({ label, enabled = true }: { label: string; enabled?: boolean }) {
  const [count, setCount] = useState(0)
  useTool('counter.increment', {
    description: '计数加一',
    enabled,
    handler: () => {
      setCount((c) => c + 1)
      return { label, count: count + 1 }
    },
  })
  return (
    <span data-testid="count">
      {label}:{count}
    </span>
  )
}

describe('useTool', () => {
  it('挂载时注册，卸载时注销', () => {
    const { app, unmount } = setup(<Counter label="a" />)
    expect(app.toolNames()).toEqual(['counter.increment'])
    expect(app.getTool('counter.increment')?.description).toBe('计数加一')
    unmount()
    expect(app.toolNames()).toEqual([])
    expect(app.count('tool.dispose')).toBe(1)
  })

  it('StrictMode 下最终只保留一个注册', () => {
    const { app, unmount } = setup(<Counter label="a" />, { strict: true })
    expect(app.toolNames()).toEqual(['counter.increment'])
    const registers = app.count('tool.register')
    expect(app.count('tool.dispose')).toBe(registers - 1)
    unmount()
    expect(app.toolNames()).toEqual([])
  })

  it('handler 闭包随渲染更新，但不重新注册', async () => {
    const { app, rerender, getByTestId } = setup(<Counter label="a" />)
    await act(async () => {
      expect(await app.call('counter.increment')).toEqual({ label: 'a', count: 1 })
    })
    expect(getByTestId('count').textContent).toBe('a:1')
    // 新闭包看到 count = 1
    await act(async () => {
      expect(await app.call('counter.increment')).toEqual({ label: 'a', count: 2 })
    })
    rerender(<Counter label="b" />)
    await act(async () => {
      expect(await app.call('counter.increment')).toEqual({ label: 'b', count: 3 })
    })
    expect(app.count('tool.register')).toBe(1)
    expect(app.count('tool.update')).toBe(0)
    expect(app.count('tool.setHandler')).toBeGreaterThan(0)
  })

  it('enabled 切换时调用 update，未变化时不调用', () => {
    const { app, rerender } = setup(<Counter label="a" enabled />)
    rerender(<Counter label="a" enabled />)
    expect(app.count('tool.update')).toBe(0)
    rerender(<Counter label="a" enabled={false} />)
    expect(app.events.filter((e) => e.type === 'tool.update')).toEqual([
      { type: 'tool.update', name: 'counter.increment', changes: { enabled: false } },
    ])
    expect(app.getTool('counter.increment')?.enabled).toBe(false)
    rerender(<Counter label="a" enabled />)
    expect(app.count('tool.update')).toBe(2)
    expect(app.getTool('counter.increment')?.enabled).toBe(true)
    expect(app.count('tool.register')).toBe(1)
  })

  it('每次渲染新建内容相同的 zod / JSON schema 时不 update，内容变化时 update', () => {
    function Search({ max, json }: { max: number; json?: boolean }) {
      useTool('search', {
        description: '搜索',
        input: json
          ? { type: 'object', properties: { keyword: { type: 'string', maxLength: max } } }
          : z.object({ keyword: z.string().max(max) }),
        risk: 'read',
        handler: () => [],
      })
      return null
    }
    const { app, rerender } = setup(<Search max={10} />)
    rerender(<Search max={10} />)
    rerender(<Search max={10} />)
    expect(app.count('tool.update')).toBe(0)
    rerender(<Search max={20} />)
    expect(app.count('tool.update')).toBe(1)
    const update = app.events.find((e) => e.type === 'tool.update')
    expect(update && 'changes' in update && Object.keys(update.changes)).toEqual(['input'])

    rerender(<Search max={20} json />)
    expect(app.count('tool.update')).toBe(2)
    rerender(<Search max={20} json />)
    expect(app.count('tool.update')).toBe(2)
  })

  it('description / risk / title / activation 变化时只 update 变化的字段', () => {
    function T(props: { description: string; risk: 'read' | 'write'; title?: string }) {
      useTool('t', { ...props, activation: 'background', handler: () => null })
      return null
    }
    const { app, rerender } = setup(<T description="一" risk="read" />)
    rerender(<T description="二" risk="read" title="标题" />)
    rerender(<T description="二" risk="write" title="标题" />)
    const updates = app.events.filter((e) => e.type === 'tool.update').map((e) => 'changes' in e && e.changes)
    expect(updates).toEqual([{ description: '二', title: '标题' }, { risk: 'write' }])
    expect(app.count('tool.register')).toBe(1)
  })

  it('name 变化时重新注册', () => {
    function T({ name }: { name: string }) {
      useTool(name, { description: 'x', handler: () => null })
      return null
    }
    const { app, rerender } = setup(<T name="a" />)
    rerender(<T name="b" />)
    expect(app.toolNames()).toEqual(['b'])
    expect(app.count('tool.register')).toBe(2)
    expect(app.count('tool.dispose', 'a')).toBe(1)
  })

  it('返回 ToolHandle（注册后的渲染中可用）', () => {
    const handles: (ToolHandle | null)[] = []
    function T({ n }: { n: number }) {
      handles.push(useTool('t', { description: `x${n}`, handler: () => null }))
      return null
    }
    const { rerender } = setup(<T n={1} />)
    rerender(<T n={2} />)
    expect(handles[0]).toBeNull()
    expect(handles[1]?.name).toBe('t')
  })

  it('没有 Provider 时为空操作', () => {
    function T() {
      const handle = useTool('t', { description: 'x', handler: () => null })
      useResource('r', { description: 'r', read: () => 1 })
      return <span>{handle === null ? 'none' : 'some'}</span>
    }
    const { container } = render(
      <ToolScope name="s">
        <T />
      </ToolScope>,
    )
    expect(container.textContent).toBe('none')
  })

  it('anchor 以稳定包装传入，始终解析最新元素', () => {
    const a = document.createElement('div')
    const b = document.createElement('div')
    function T({ el }: { el: Element }) {
      useTool('t', { description: 'x', anchor: () => el, handler: () => null })
      return null
    }
    const { app, rerender } = setup(<T el={a} />)
    const anchor = app.getTool('t')?.anchor as () => Element | null
    expect(anchor()).toBe(a)
    rerender(<T el={b} />)
    expect(anchor()).toBe(b)
    expect(app.count('tool.update')).toBe(0)
  })
})

describe('useTool 惰性 handler', () => {
  function Lazy({ label, load }: { label: string; load: () => Promise<{ default: (i: { n: number }) => string }> }) {
    useTool('lazy.tool', { description: label, input: { type: 'object', properties: { n: { type: 'number' } } }, load })
    return null
  }

  it('透传 load，重渲染不调用 setHandler、不重新注册；首次调用时加载', async () => {
    let loads = 0
    const load = async () => {
      loads++
      return { default: (i: { n: number }) => `n=${i.n}` }
    }
    const { app, rerender } = setup(<Lazy label="a" load={load} />)
    expect(app.getTool('lazy.tool')?.load).toBe(load)
    expect(app.getTool('lazy.tool')?.handler).toBeUndefined()
    rerender(<Lazy label="a" load={async () => ({ default: () => 'other' })} />)
    rerender(<Lazy label="b" load={load} />)
    expect(app.count('tool.register')).toBe(1)
    expect(app.count('tool.setHandler')).toBe(0)
    expect(app.count('tool.update')).toBe(1)
    expect(loads).toBe(0)
    await act(async () => {
      expect(await app.call('lazy.tool', { n: 3 })).toBe('n=3')
    })
    expect(loads).toBe(1)
  })
})

describe('useResource', () => {
  function Todos({ items }: { items: string[] }) {
    useResource('todos.list', {
      description: '待办列表',
      read: () => ({ items }),
      deps: [items],
    })
    return null
  }

  it('注册、读取最新内容、deps 变化时通知', async () => {
    const first = ['a']
    const { app, rerender, unmount } = setup(<Todos items={first} />)
    expect(app.resourceNames()).toEqual(['todos.list'])
    expect(app.count('resource.notify')).toBe(0)
    rerender(<Todos items={first} />)
    expect(app.count('resource.notify')).toBe(0)
    const second = ['a', 'b']
    rerender(<Todos items={second} />)
    expect(app.count('resource.notify')).toBe(1)
    expect(await app.read('todos.list')).toEqual({ items: ['a', 'b'] })
    expect(app.count('resource.register')).toBe(1)
    unmount()
    expect(app.resourceNames()).toEqual([])
  })

  it('StrictMode 下首次挂载不通知，最终只保留一个注册', () => {
    const { app } = setup(<Todos items={['a']} />, { strict: true })
    expect(app.resourceNames()).toEqual(['todos.list'])
    expect(app.count('resource.notify')).toBe(0)
    expect(app.count('resource.dispose')).toBe(app.count('resource.register') - 1)
  })

  it('description 变化时重新注册', () => {
    function R({ d }: { d: string }) {
      useResource('r', { description: d, mimeType: 'text/plain', read: () => 'x' })
      return null
    }
    const { app, rerender } = setup(<R d="一" />)
    rerender(<R d="二" />)
    expect(app.count('resource.register')).toBe(2)
    expect(app.resourceNames()).toEqual(['r'])
  })
})

describe('ToolScope', () => {
  function Tool({ name }: { name: string }) {
    useTool(name, { description: name, handler: () => null })
    return null
  }

  it('子组件的工具注册在子 scope 中，卸载时整体注销', () => {
    function Page({ show }: { show: boolean }) {
      return show ? (
        <ToolScope name="cart">
          <Tool name="cart.add" />
          <ToolScope name="inner">
            <Tool name="cart.inner" />
          </ToolScope>
        </ToolScope>
      ) : null
    }
    const { app, rerender } = setup(
      <>
        <Tool name="root" />
        <Page show />
      </>,
    )
    expect(app.toolNames()).toEqual(['cart.add', 'cart.inner', 'root'])
    expect(app.getToolScope('cart.add')).toBe('cart')
    expect(app.getToolScope('cart.inner')).toBe('inner')
    expect(app.getToolScope('root')).toBeNull()
    expect(app.liveScopes).toBe(2)

    rerender(
      <>
        <Tool name="root" />
        <Page show={false} />
      </>,
    )
    expect(app.toolNames()).toEqual(['root'])
    expect(app.liveScopes).toBe(0)
  })

  it('StrictMode 下 scope 与工具最终各只剩一个', () => {
    const { app, unmount } = setup(
      <ToolScope name="outer">
        <Tool name="a" />
        <ToolScope name="inner">
          <Tool name="b" />
        </ToolScope>
      </ToolScope>,
      { strict: true },
    )
    expect(app.toolNames()).toEqual(['a', 'b'])
    expect(app.liveScopes).toBe(2)
    expect(app.getToolScope('b')).toBe('inner')
    unmount()
    expect(app.toolNames()).toEqual([])
    expect(app.liveScopes).toBe(0)
  })

  it('没有工具时不创建 scope', () => {
    const { app } = setup(
      <ToolScope name="empty">
        <span />
      </ToolScope>,
    )
    expect(app.liveScopes).toBe(0)
  })

  it('反复挂载卸载后注册表为空', () => {
    function Toggle({ on }: { on: boolean }) {
      return on ? (
        <ToolScope name="s">
          <Tool name="x" />
        </ToolScope>
      ) : null
    }
    const { app, rerender } = setup(<Toggle on />, { strict: true })
    for (let i = 0; i < 50; i++) {
      rerender(<Toggle on={false} />)
      rerender(<Toggle on />)
    }
    rerender(<Toggle on={false} />)
    expect(app.toolNames()).toEqual([])
    expect(app.liveScopes).toBe(0)
  })
})

describe('零侵入', () => {
  it('接入 hooks 前后渲染次数一致', async () => {
    const commits = { plain: 0, withMcp: 0 }
    function Plain({ n }: { n: number }) {
      const [items] = useState(() => ['a'])
      return (
        <span>
          {n}
          {items.length}
        </span>
      )
    }
    function WithMcp({ n }: { n: number }) {
      const [items] = useState(() => ['a'])
      useTool('t', { description: `d${n % 2}`, enabled: n % 3 !== 0, handler: () => n })
      useResource('r', { description: 'r', read: () => items, deps: [n] })
      return (
        <ToolScope name="s">
          <span>
            {n}
            {items.length}
          </span>
        </ToolScope>
      )
    }
    function App({ n }: { n: number }) {
      return (
        <>
          <Profiler id="plain" onRender={() => commits.plain++}>
            <Plain n={n} />
          </Profiler>
          <Profiler id="withMcp" onRender={() => commits.withMcp++}>
            <WithMcp n={n} />
          </Profiler>
        </>
      )
    }
    const { app, rerender } = setup(<App n={1} />)
    for (let n = 2; n <= 7; n++) rerender(<App n={n} />)
    // 模型调用 handler、读取资源不会触发渲染
    await act(async () => {
      await app.call('t')
      await app.read('r')
    })
    expect(commits.withMcp).toBe(commits.plain)
    expect(commits.plain).toBe(7)
  })
})

describe('useAppMcp / useConnectionState', () => {
  it('useAppMcp 在 Provider 外抛错', () => {
    function T() {
      useAppMcp()
      return null
    }
    const spy = console.error
    console.error = () => {}
    try {
      expect(() => render(<T />)).toThrow(/AppMcpProvider/)
    } finally {
      console.error = spy
    }
  })

  it('useConnectionState 跟随状态变化', () => {
    function Status() {
      return <span>{useConnectionState().status}</span>
    }
    const { app, container } = setup(<Status />)
    expect(container.textContent).toBe('idle')
    act(() => app.setState({ status: 'connected' }))
    expect(container.textContent).toBe('connected')
    act(() => app.setState({ status: 'dormant' }))
    expect(container.textContent).toBe('dormant')
    act(() => app.setState({ status: 'waking' }))
    expect(container.textContent).toBe('waking')
  })
})

describe('useHold', () => {
  function Holder({ active }: { active?: boolean }) {
    useHold(active)
    return null
  }

  it('挂载期间持有，卸载时释放', () => {
    const { app, rerender } = setup(<Holder />)
    expect(app.activeHolds).toBe(1)
    rerender(<div />)
    expect(app.activeHolds).toBe(0)
  })

  it('active 切换时获取 / 释放', () => {
    const { app, rerender } = setup(<Holder active={false} />)
    expect(app.activeHolds).toBe(0)
    rerender(<Holder active />)
    expect(app.activeHolds).toBe(1)
    rerender(<Holder active={false} />)
    expect(app.activeHolds).toBe(0)
  })

  it('StrictMode 下不泄漏持有', () => {
    const { app, rerender } = setup(<Holder />, { strict: true })
    expect(app.activeHolds).toBe(1)
    rerender(<div />)
    expect(app.activeHolds).toBe(0)
  })
})
