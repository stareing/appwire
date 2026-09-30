/**
 * 模拟第三方 React Hook 库（use-webmcp-tool、@mcp-b/react-webmcp、webmcp-react 等）的调用序列：
 * 挂载时 registerTool、卸载时注销。不依赖 React，用纯函数模拟 useEffect 的 setup / cleanup。
 */

import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { installWebMcp, type ModelContext, type ModelContextTool, type WebMcpUninstall } from '../src/webmcp'
import { connected, type Harness, settle } from './fakes'
import { coreTool, defineNative, FakeNativeModelContext, hostCall } from './webmcp-fakes'

type Effect = () => () => void

/** 最小的"组件"：模拟 useEffect 的挂载 / 卸载 / 依赖变化，以及 StrictMode 的双重挂载。 */
class FakeComponent {
  private cleanup: (() => void) | undefined
  constructor(private effect: Effect) {}
  mount(strict = false): void {
    this.cleanup = this.effect()
    if (strict) {
      // React 18+ StrictMode：开发模式下挂载 → 卸载 → 再挂载
      this.cleanup()
      this.cleanup = this.effect()
    }
  }
  rerender(effect: Effect): void {
    this.cleanup?.()
    this.effect = effect
    this.cleanup = effect()
  }
  unmount(): void {
    this.cleanup?.()
    this.cleanup = undefined
  }
}

const mcOf = (owner: object): ModelContext => (owner as { modelContext: ModelContext }).modelContext

/** 等价于 use-webmcp-tool / webmcp-react（当前标准写法）：document.modelContext + AbortSignal。 */
function useWebMcpTool(tool: ModelContextTool): Effect {
  return () => {
    const ac = new AbortController()
    mcOf(document)
      .registerTool(tool, { signal: ac.signal })
      .catch(() => {})
    return () => ac.abort()
  }
}

/** 等价于早期 @mcp-b/react-webmcp（旧写法）：navigator.modelContext + unregisterTool。 */
function useLegacyTool(tool: ModelContextTool): Effect {
  return () => {
    void mcOf(navigator).registerTool(tool).catch(() => {})
    return () => mcOf(navigator).unregisterTool(tool.name)
  }
}

/** 更早的写法：provideContext / clearContext。 */
function useProvideContext(tools: ModelContextTool[]): Effect {
  return () => {
    mcOf(navigator).provideContext({ tools })
    return () => mcOf(navigator).clearContext()
  }
}

const hooks = { useWebMcpTool, useLegacyTool } as const

for (const mode of ['polyfill', 'bridge'] as const) {
  describe(`第三方 Hook 库（${mode}）`, () => {
    let h: Harness
    let uninstall: WebMcpUninstall | undefined
    let native: FakeNativeModelContext | undefined
    let undoNative: (() => void) | undefined

    beforeEach(async () => {
      sessionStorage.clear()
      localStorage.clear()
      h = await connected()
      if (mode === 'bridge') {
        native = new FakeNativeModelContext()
        undoNative = defineNative(document, native)
      }
      uninstall = installWebMcp(h.app)
      expect(uninstall.mode).toBe(mode)
    })

    afterEach(() => {
      uninstall?.()
      undoNative?.()
      native = undefined
      undoNative = undefined
      h.app.dispose()
    })

    for (const [hookName, useTool] of Object.entries(hooks)) {
      it(`${hookName}：挂载注册、卸载注销、StrictMode 双重挂载后只剩一份`, async () => {
        let count = 0
        const tool = (description: string): ModelContextTool => ({
          name: 'counter.increment',
          description,
          inputSchema: { type: 'object', properties: { by: { type: 'number' } } },
          annotations: { readOnlyHint: false },
          execute: async ({ by }: { by: number }) => {
            count += by
            return { content: [{ type: 'text', text: JSON.stringify({ count }) }] }
          },
        })
        const comp = new FakeComponent(useTool(tool('计数 +n')))
        comp.mount(true)
        await settle()
        expect(coreTool(h, 'counter.increment')?.description).toBe('计数 +n')
        expect(h.core.callsOf('registerTool').filter(([d]) => (d as { name: string }).name === 'counter.increment')).toHaveLength(2)
        if (native) expect([...native.tools.keys()]).toEqual(['counter.increment'])

        expect(await hostCall(h, 'counter.increment', { by: 2 })).toEqual({ data: { count: 2 } })

        // 依赖变化：cleanup + 重新注册
        comp.rerender(useTool(tool('计数 +n（新）')))
        await settle()
        expect(coreTool(h, 'counter.increment')?.description).toBe('计数 +n（新）')
        expect(await hostCall(h, 'counter.increment', { by: 3 })).toEqual({ data: { count: 5 } })

        comp.unmount()
        await settle()
        expect(coreTool(h, 'counter.increment')).toBeUndefined()
        if (native) expect(native.tools.size).toBe(0)
      })
    }

    it('useProvideContext：多个工具一起提供与清除，不影响 appMcp.tool()', async () => {
      h.app.tool('own', { description: '自有', handler: () => 1 })
      const comp = new FakeComponent(
        useProvideContext([
          { name: 'a', description: 'A', execute: () => 'a' },
          { name: 'b', description: 'B', execute: () => 'b' },
        ]),
      )
      comp.mount(true)
      await settle()
      expect(coreTool(h, 'a')).toBeDefined()
      expect(await hostCall(h, 'b')).toEqual({ data: { text: 'b' } })
      comp.unmount()
      await settle()
      expect(coreTool(h, 'a')).toBeUndefined()
      expect(coreTool(h, 'b')).toBeUndefined()
      expect(coreTool(h, 'own')).toBeDefined()
      if (native) expect([...native.tools.keys()]).toEqual(['own'])
    })

    it('两个组件注册不同工具，各自卸载互不影响', async () => {
      const a = new FakeComponent(useWebMcpTool({ name: 'a', description: 'A', execute: () => 1 }))
      const b = new FakeComponent(useLegacyTool({ name: 'b', description: 'B', execute: () => 2 }))
      a.mount()
      b.mount()
      await settle()
      a.unmount()
      await settle()
      expect(coreTool(h, 'a')).toBeUndefined()
      expect(await hostCall(h, 'b')).toEqual({ data: 2 })
      b.unmount()
    })
  })
}
