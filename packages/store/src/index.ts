/**
 * @app-mcp/store：把状态库的 action 暴露为 MCP 工具、把 state 切片暴露为资源。
 *
 * 工具挂在状态层而不是 UI 层：按钮调用的是同一个 action，人和模型走同一条路径。
 * 本模块是与具体状态库无关的通用核心；Zustand / Redux / Pinia 的适配见各子路径。
 *
 * ```ts
 * import { exposeStore } from '@app-mcp/store'
 * const dispose = exposeStore(appMcp, { getState, subscribe }, { actions: { ... } })
 * ```
 */

import {
  ToolCallError,
  type Activation,
  type InputDefinition,
  type Registrar,
  type ResourceHandle,
  type Risk,
  type ToolDefinition,
  type ToolHandle,
} from '@app-mcp/web'

// ---------------------------------------------------------------------------
// 类型
// ---------------------------------------------------------------------------

/** 一次 action 调用的上下文，交给适配器执行。 */
export interface ActionCall<S = unknown> {
  /** 完整工具名（含 namespace）。 */
  tool: string
  /** action 名：`options.action`，缺省为工具名最后一段。 */
  action: string
  /** 经 `args` 映射后的位置参数。 */
  args: unknown[]
  /** 原始参数对象。 */
  input: unknown
  /** 该工具的配置。 */
  config: ActionOptions<S>
}

/**
 * 状态库适配器的最小结构。只要求 `getState` 与 `subscribe`。
 */
export interface StoreAdapter<S> {
  getState(): S
  /** 订阅任意变化，返回取消订阅函数。 */
  subscribe(listener: () => void): () => void
  /**
   * 可选：执行 action。缺省在 `getState()` 上按 action 名调用函数（Zustand 风格）。
   * 可以返回 Promise。
   */
  invoke?(call: ActionCall<S>): unknown
  /**
   * 可选：state 被原地修改（如 Pinia / Vue 响应式）。为 true 时资源变化检测基于 JSON 快照，
   * 因为选中值的引用可能不变而内容已变。
   */
  mutable?: boolean
}

export interface ActionOptions<S> {
  description: string
  title?: string
  /** JSON Schema、zod v4 schema 或带 `toJSONSchema()` 的对象；缺省为无参数。 */
  input?: InputDefinition<any>
  risk?: Risk
  activation?: Activation
  /** Zustand / Pinia：state（store）上的函数名；Redux：未给 `creator` 时作为 action type。缺省为工具名最后一段。 */
  action?: string
  /** Redux：action creator（普通 action、thunk 或 createAsyncThunk）。 */
  creator?: (...args: any[]) => any
  /** 把参数对象映射为位置参数。缺省：声明了 `input` 时为 `[input]`，否则为 `[]`。 */
  args?: (input: any) => unknown[]
  /** 根据 state 决定工具是否可用；状态变化时重新计算，只有结果变化才会更新。 */
  enabled?: (state: S) => boolean
  /** 结果选择器：action 完成后以最新 state 与 action 返回值计算结果。缺省返回 action 返回值（undefined 时为 `{ ok: true }`）。 */
  result?: (state: S, returned: unknown) => unknown
  /** 附带在结果中的 stateHints。 */
  hints?: string[]
}

export interface ResourceOptions<S> {
  description: string
  /** 从 state 中选出资源内容（应可 JSON 序列化）。 */
  select: (state: S) => unknown
  /** 判断选中值是否相同；缺省为浅比较（mutable 适配器缺省比较 JSON 快照）。 */
  equals?: (a: unknown, b: unknown) => boolean
}

export interface ExposeOptions<S> {
  /** 工具名与资源名前缀，如 `'cart'` → `'cart.add'`。 */
  namespace?: string
  actions: Record<string, ActionOptions<S>>
  resources?: Record<string, ResourceOptions<S>>
}

/** 注销全部工具与资源并取消订阅。可重复调用。 */
export type Dispose = () => void

// ---------------------------------------------------------------------------
// 工具函数
// ---------------------------------------------------------------------------

export function isPromiseLike(value: unknown): value is PromiseLike<unknown> {
  return (
    (typeof value === 'object' || typeof value === 'function') &&
    value !== null &&
    typeof (value as { then?: unknown }).then === 'function'
  )
}

/** 浅比较：`Object.is`，或两个数组 / 普通对象的一层元素逐个 `Object.is`。 */
export function shallowEqual(a: unknown, b: unknown): boolean {
  if (Object.is(a, b)) return true
  if (typeof a !== 'object' || typeof b !== 'object' || a === null || b === null) return false
  if (Array.isArray(a) !== Array.isArray(b)) return false
  if (Array.isArray(a)) {
    const bb = b as unknown[]
    return a.length === bb.length && a.every((v, i) => Object.is(v, bb[i]))
  }
  if (Object.getPrototypeOf(a) !== Object.getPrototypeOf(b)) return false
  const ka = Object.keys(a)
  const kb = Object.keys(b)
  if (ka.length !== kb.length) return false
  const ra = a as Record<string, unknown>
  const rb = b as Record<string, unknown>
  return ka.every((k) => Object.prototype.hasOwnProperty.call(rb, k) && Object.is(ra[k], rb[k]))
}

function joinName(namespace: string | undefined, name: string): string {
  return namespace ? `${namespace}.${name}` : name
}

/** 工具对应的 action 名：`config.action`，缺省为工具名最后一段（`'cart.add'` → `'add'`）。 */
export function actionNameOf(toolName: string, config: { action?: string }): string {
  return config.action ?? toolName.slice(toolName.lastIndexOf('.') + 1)
}

function evalEnabled<S>(fn: (state: S) => boolean, state: S): boolean {
  try {
    return Boolean(fn(state))
  } catch {
    // enabled 计算出错时视为不可用，避免模型调用处于异常状态的 action。
    return false
  }
}

/** 默认执行方式：在 state 上按名称调用函数（Zustand 风格）。 */
function invokeOnState<S>(adapter: StoreAdapter<S>, call: ActionCall<S>): unknown {
  const target = adapter.getState() as Record<string, unknown> | null
  const fn = target?.[call.action]
  if (typeof fn !== 'function') throw new Error(`state 上不存在函数 ${call.action}（工具 ${call.tool}）`)
  return (fn as (...args: unknown[]) => unknown).apply(target, call.args)
}

/** mutable 适配器的快照：JSON 字符串用于比较，克隆值交给自定义 equals。 */
interface Snapshot {
  value: unknown
  key: string | undefined
}

function snapshot(value: unknown, mutable: boolean | undefined): Snapshot {
  if (!mutable) return { value, key: undefined }
  let key: string | undefined
  try {
    key = JSON.stringify(value)
  } catch {
    key = undefined
  }
  return { value: key === undefined ? value : JSON.parse(key), key }
}

// ---------------------------------------------------------------------------
// 核心
// ---------------------------------------------------------------------------

interface ToolEntry<S> {
  handle: ToolHandle
  enabledFn: ((state: S) => boolean) | undefined
  lastEnabled: boolean
}

interface ResourceEntry<S> {
  handle: ResourceHandle
  options: ResourceOptions<S>
  last: Snapshot
}

/**
 * 把 store 的 action 注册为工具、state 切片注册为资源。
 *
 * - 同一个 store 订阅负责 `enabled` 重算与资源变化检测，每个微任务最多处理一次；
 * - 调用前再次检查 `enabled`，不可用时抛出 `TOOL_DISABLED`；
 * - action 抛出的 `ToolCallError` 原样透传，其他异常由 SDK 归为 `HANDLER_ERROR`。
 */
export function exposeStore<S>(registrar: Registrar, adapter: StoreAdapter<S>, options: ExposeOptions<S>): Dispose {
  const tools: ToolEntry<S>[] = []
  const resources: ResourceEntry<S>[] = []
  let unsubscribe: (() => void) | undefined
  let disposed = false
  let scheduled = false

  const dispose: Dispose = () => {
    if (disposed) return
    disposed = true
    unsubscribe?.()
    unsubscribe = undefined
    for (const t of tools) t.handle.dispose()
    for (const r of resources) r.handle.dispose()
    tools.length = 0
    resources.length = 0
  }

  const invoke = adapter.invoke ? adapter.invoke.bind(adapter) : (call: ActionCall<S>) => invokeOnState(adapter, call)

  try {
    const initial = adapter.getState()

    for (const [toolName, config] of Object.entries(options.actions)) {
      const name = joinName(options.namespace, toolName)
      const action = actionNameOf(toolName, config)
      const enabledFn = config.enabled
      const initiallyEnabled = enabledFn ? evalEnabled(enabledFn, initial) : true

      const definition: ToolDefinition<unknown, unknown> = {
        description: config.description,
        handler: async (input) => {
          if (enabledFn && !evalEnabled(enabledFn, adapter.getState())) {
            throw new ToolCallError('TOOL_DISABLED', `工具 ${name} 当前不可用`)
          }
          const args = config.args ? config.args(input) : config.input !== undefined ? [input] : []
          let returned = invoke({ tool: name, action, args, input, config })
          if (isPromiseLike(returned)) returned = await returned
          const data = config.result
            ? config.result(adapter.getState(), returned)
            : returned === undefined
              ? { ok: true }
              : returned
          // 始终用 { data } 包装，避免返回值本身形如 { data } 时被 SDK 误拆。
          return config.hints && config.hints.length > 0 ? { data, stateHints: config.hints } : { data }
        },
      }
      if (config.title !== undefined) definition.title = config.title
      if (config.input !== undefined) definition.input = config.input
      if (config.risk !== undefined) definition.risk = config.risk
      if (config.activation !== undefined) definition.activation = config.activation
      if (enabledFn) definition.enabled = initiallyEnabled

      tools.push({ handle: registrar.tool(name, definition), enabledFn, lastEnabled: initiallyEnabled })
    }

    for (const [resName, res] of Object.entries(options.resources ?? {})) {
      const name = joinName(options.namespace, resName)
      const handle = registrar.resource(name, {
        description: res.description,
        read: () => res.select(adapter.getState()),
      })
      resources.push({ handle, options: res, last: snapshot(res.select(initial), adapter.mutable) })
    }
  } catch (e) {
    dispose()
    throw e
  }

  const process = (): void => {
    scheduled = false
    if (disposed) return
    const state = adapter.getState()
    for (const t of tools) {
      if (!t.enabledFn) continue
      const next = evalEnabled(t.enabledFn, state)
      if (next === t.lastEnabled) continue
      t.lastEnabled = next
      // 只传 enabled：update 中显式为 undefined 的字段会被重置为默认值。
      t.handle.update({ enabled: next })
    }
    for (const r of resources) {
      let next: Snapshot
      try {
        next = snapshot(r.options.select(state), adapter.mutable)
      } catch {
        continue
      }
      const equals = r.options.equals
      const same = equals
        ? equals(r.last.value, next.value)
        : adapter.mutable && r.last.key !== undefined && next.key !== undefined
          ? r.last.key === next.key
          : shallowEqual(r.last.value, next.value)
      if (same) continue
      r.last = next
      r.handle.notifyChanged()
    }
  }

  if (tools.some((t) => t.enabledFn) || resources.length > 0) {
    unsubscribe = adapter.subscribe(() => {
      if (scheduled || disposed) return
      scheduled = true
      queueMicrotask(process)
    })
  }

  return dispose
}

export type { Registrar } from '@app-mcp/web'
