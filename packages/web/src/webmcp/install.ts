/**
 * `installWebMcp(appMcp)`：让按 W3C WebMCP 标准写的代码把工具接入 app-mcp，
 * 并把 `appMcp.tool()` 的工具同步给浏览器内置 AI。
 */

import { getToolHub } from '../tool-hub'
import type { AppMcp, Logger } from '../types'
import { type NativeApi, StandardBridge } from './bridge'
import { ToolActivatedEvent, ToolCancelEvent } from './events'
import { ModelContextPolyfill } from './polyfill'
import type { ModelContext, WebMcpMode, WebMcpOptions, WebMcpUninstall } from './types'

/** 在原生对象上覆盖（或补充）的方法。 */
const WRAPPED_METHODS = ['registerTool', 'unregisterTool', 'provideContext', 'clearContext'] as const

const installed = new WeakMap<object, true>()

const consoleLogger: Logger = {
  debug: () => {},
  warn: (message, ...args) => console.warn(message, ...args),
  error: (message, ...args) => console.error(message, ...args),
}

type AnyRecord = Record<string, unknown>

function isModelContextLike(v: unknown): v is EventTarget & AnyRecord {
  return typeof v === 'object' && v !== null && typeof (v as AnyRecord).registerTool === 'function'
}

function readModelContext(owner: object | undefined): unknown {
  if (!owner) return undefined
  try {
    return (owner as AnyRecord).modelContext
  } catch {
    return undefined
  }
}

/** 在 owner 上定义 `modelContext`，返回撤销函数；失败返回 undefined。 */
function defineModelContext(owner: object, value: object): (() => void) | undefined {
  const prev = Object.getOwnPropertyDescriptor(owner, 'modelContext')
  try {
    Object.defineProperty(owner, 'modelContext', { configurable: true, enumerable: true, get: () => value })
  } catch {
    return undefined
  }
  if ((owner as AnyRecord).modelContext !== value) {
    restoreProperty(owner, 'modelContext', prev)
    return undefined
  }
  return () => {
    if (Object.getOwnPropertyDescriptor(owner, 'modelContext')?.get?.() === value) {
      restoreProperty(owner, 'modelContext', prev)
    }
  }
}

function restoreProperty(owner: object, key: string, prev: PropertyDescriptor | undefined): void {
  try {
    if (prev) Object.defineProperty(owner, key, prev)
    else delete (owner as AnyRecord)[key]
  } catch {
    // 忽略
  }
}

/** 在全局补充 ToolActivatedEvent / ToolCancelEvent 构造函数（仅 polyfill 模式，且原来不存在时）。 */
function defineEventClasses(): () => void {
  const g = globalThis as AnyRecord
  const undo: (() => void)[] = []
  for (const [key, ctor] of [
    ['ToolActivatedEvent', ToolActivatedEvent],
    ['ToolCancelEvent', ToolCancelEvent],
  ] as const) {
    if (key in g) continue
    try {
      Object.defineProperty(g, key, { configurable: true, writable: true, value: ctor })
      undo.push(() => {
        if (g[key] === ctor) delete g[key]
      })
    } catch {
      // 忽略
    }
  }
  return () => undo.forEach((f) => f())
}

/**
 * 包装原生对象的方法（覆盖实例属性，不替换对象本身）。成功返回撤销函数，失败返回 undefined（已回滚）。
 */
function wrapNative(native: AnyRecord, bridge: StandardBridge): (() => void) | undefined {
  const saved = new Map<string, PropertyDescriptor | undefined>()
  const impl: AnyRecord = {
    registerTool: bridge.registerTool.bind(bridge),
    unregisterTool: bridge.unregisterTool.bind(bridge),
    provideContext: bridge.provideContext.bind(bridge),
    clearContext: bridge.clearContext.bind(bridge),
  }
  const restore = (): void => {
    for (const [key, prev] of saved) {
      if (Object.getOwnPropertyDescriptor(native, key)?.value === impl[key]) restoreProperty(native, key, prev)
    }
  }
  try {
    for (const key of WRAPPED_METHODS) {
      saved.set(key, Object.getOwnPropertyDescriptor(native, key))
      Object.defineProperty(native, key, { configurable: true, writable: true, enumerable: false, value: impl[key] })
      if (native[key] !== impl[key]) throw new Error(`无法覆盖 ${key}`)
    }
  } catch {
    restore()
    return undefined
  }
  return restore
}

/**
 * 安装 WebMCP 支持。
 *
 * - **没有原生支持（polyfill）**：在 `document.modelContext` 与 `navigator.modelContext` 上安装
 *   标准兼容的对象；`registerTool()` 等调用转为 `appMcp.tool()`。
 * - **有原生支持（桥接）**：包装原生对象的 `registerTool` / `unregisterTool` / `provideContext` /
 *   `clearContext`，页面注册的工具同时进入原生（浏览器内置 AI）与 app-mcp（Host）。
 * - `mirrorOwnTools`（默认 true）：`appMcp.tool()` 注册的工具也注册到原生 modelContext。
 *
 * 返回卸载函数（带 `mode` 与 `modelContext` 属性）。
 */
export function installWebMcp(appMcp: AppMcp, options: WebMcpOptions = {}): WebMcpUninstall {
  const doc = options.document ?? (typeof document === 'undefined' ? undefined : document)
  if (!doc) throw new Error('installWebMcp 需要浏览器环境（document）')
  const nav = options.navigator ?? (typeof navigator === 'undefined' ? undefined : navigator)
  if (installed.has(doc)) throw new Error('WebMCP 已安装到此文档，请先调用上次返回的卸载函数')

  const log = options.logger ?? appMcp.options.logger ?? consoleLogger
  const mirrorOwnTools = options.mirrorOwnTools ?? true
  const hub = getToolHub(appMcp)

  const docMc = readModelContext(doc)
  const navMc = readModelContext(nav)
  const native = isModelContextLike(docMc) ? docMc : isModelContextLike(navMc) ? navMc : undefined

  const undo: (() => void)[] = []
  let mode: WebMcpMode
  let modelContext: ModelContext
  let bridge: StandardBridge

  if (!native) {
    // ---- polyfill ----
    let poly: ModelContextPolyfill | undefined
    let pendingChange = false
    const onChange = (): void => {
      if (!poly || pendingChange) return
      pendingChange = true
      queueMicrotask(() => {
        pendingChange = false
        poly?.dispatchEvent(new Event('toolchange'))
      })
    }
    bridge = new StandardBridge(appMcp, hub, log, mirrorOwnTools, undefined, onChange)
    poly = new ModelContextPolyfill(bridge)
    bridge.target = poly
    const undoDoc = defineModelContext(doc, poly)
    if (!undoDoc) throw new Error('无法在 document 上安装 modelContext')
    undo.push(undoDoc)
    if (nav) {
      const undoNav = defineModelContext(nav, poly)
      if (undoNav) undo.push(undoNav)
      else log.warn('[app-mcp] 无法在 navigator 上安装 modelContext')
    }
    undo.push(defineEventClasses())
    mode = 'polyfill'
    modelContext = poly
  } else {
    // ---- 桥接 ----
    const register = native.registerTool as (tool: object, options: object) => unknown
    const unregister = native.unregisterTool
    const api: NativeApi = {
      target: native,
      registerTool: (tool, opts) => register.call(native, tool, opts),
      ...(typeof unregister === 'function' && {
        unregisterTool: (name: string) => (unregister as (n: string) => unknown).call(native, name),
      }),
    }
    bridge = new StandardBridge(appMcp, hub, log, mirrorOwnTools, api, () => {})
    bridge.target = native
    const restore = wrapNative(native, bridge)
    if (restore) {
      undo.push(restore)
      mode = 'bridge'
    } else {
      log.warn('[app-mcp] 原生 modelContext 的方法无法覆盖：经原生接口注册的工具不会进入 app-mcp，仅镜像 appMcp.tool() 的工具')
      mode = 'native-readonly'
    }
    // 另一个入口缺失时补上别名（Chrome 从 navigator 迁移到 document）
    if (!isModelContextLike(docMc)) {
      const u = defineModelContext(doc, native)
      if (u) undo.push(u)
    } else if (nav && navMc === undefined) {
      const u = defineModelContext(nav, native)
      if (u) undo.push(u)
    }
    modelContext = native as unknown as ModelContext
  }

  bridge.startMirroring()
  installed.set(doc, true)

  let done = false
  const uninstall = (): void => {
    if (done) return
    done = true
    bridge.close()
    for (const f of undo.reverse()) f()
    installed.delete(doc)
  }
  return Object.assign(uninstall, { mode, modelContext })
}
