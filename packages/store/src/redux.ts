/**
 * Redux 适配：action 通过 `store.dispatch(creator(...args))` 执行。
 *
 * - 普通 action creator：dispatch 返回 action 本身，视为无返回值；
 * - thunk：dispatch 返回 thunk 的返回值，Promise 会被等待；
 * - `createAsyncThunk`：按 `unwrap()` 语义取 fulfilled 的 payload；rejected 时转换为错误
 *   （`rejectWithValue({ kind, message })` 或带合法 `code` 的错误 → ToolCallError，其余 → HANDLER_ERROR）。
 *
 * ```ts
 * import { exposeRedux } from '@app-mcp/store/redux'
 * exposeRedux(appMcp, store, { namespace: 'cart', actions: { add: { description: '加入购物车', input, creator: cartSlice.actions.add } } })
 * ```
 */

import { ToolCallError, type ErrorKind, type Registrar } from '@app-mcp/web'
import { type Dispose, type ExposeOptions, exposeStore, isPromiseLike } from './index'

/** Redux store（含 RTK `configureStore` 的返回值）的最小结构。 */
export interface ReduxStoreLike<S> {
  getState(): S
  subscribe(listener: () => void): () => void
  dispatch(action: any): any
}

const ERROR_KINDS: ReadonlySet<string> = new Set<ErrorKind>([
  'TOOL_NOT_FOUND',
  'TOOL_DISABLED',
  'INVALID_INPUT',
  'USER_REJECTED',
  'TIMEOUT',
  'HANDLER_ERROR',
  'CANCELLED',
  'APP_DISCONNECTED',
  'APP_NOT_INSTALLED',
  'LAUNCH_FAILED',
  'APP_NOT_RESPONDING',
  'INSTANCE_FROZEN',
  'RESOURCE_NOT_FOUND',
  'UNAUTHORIZED',
  'UNSUPPORTED_PROTOCOL',
])

/** createAsyncThunk dispatch 结果：带 `unwrap()` 的 Promise。 */
function isAsyncThunkPromise(value: unknown): value is PromiseLike<unknown> & { unwrap(): Promise<unknown> } {
  return isPromiseLike(value) && typeof (value as { unwrap?: unknown }).unwrap === 'function'
}

function describe(value: unknown): string {
  if (typeof value === 'string') return value
  try {
    return JSON.stringify(value) ?? String(value)
  } catch {
    return String(value)
  }
}

/** 把异步 thunk 的 rejected 原因转换为交给 SDK 的错误。 */
export function toCallError(reason: unknown): Error {
  if (reason instanceof ToolCallError) return reason
  if (typeof reason === 'object' && reason !== null) {
    const r = reason as { kind?: unknown; code?: unknown; message?: unknown; details?: unknown }
    const message = typeof r.message === 'string' && r.message ? r.message : '异步 action 失败'
    const details =
      typeof r.details === 'object' && r.details !== null ? (r.details as Record<string, unknown>) : undefined
    // rejectWithValue({ kind, message }) 或 ToolCallError 实例（跨 realm 等 instanceof 失效时）
    if (typeof r.kind === 'string' && ERROR_KINDS.has(r.kind)) {
      return new ToolCallError(r.kind as ErrorKind, message, details)
    }
    // createAsyncThunk 序列化后的错误（SerializedError）：code 为合法类别时保留
    if (typeof r.code === 'string' && ERROR_KINDS.has(r.code)) {
      return new ToolCallError(r.code as ErrorKind, message, details)
    }
    if (reason instanceof Error) return reason
    if (typeof r.message === 'string') return new Error(message)
  }
  return new Error(`异步 action 失败：${describe(reason)}`)
}

export function exposeRedux<S>(registrar: Registrar, store: ReduxStoreLike<S>, options: ExposeOptions<S>): Dispose {
  for (const [toolName, config] of Object.entries(options.actions)) {
    if (typeof config.creator !== 'function' && typeof config.action !== 'string') {
      throw new Error(`Redux 工具 ${toolName} 需要提供 creator（action creator / thunk）或 action（action type）`)
    }
  }
  return exposeStore<S>(
    registrar,
    {
      getState: () => store.getState(),
      subscribe: (listener) => store.subscribe(listener),
      invoke: (call) => {
        const { creator } = call.config
        const action = creator
          ? creator(...call.args)
          : call.args.length > 0
            ? { type: call.action, payload: call.args[0] }
            : { type: call.action }
        const ret: unknown = store.dispatch(action)
        // 普通 action：dispatch 原样返回该 action，没有有意义的返回值。
        if (ret === action && typeof action === 'object') return undefined
        if (isAsyncThunkPromise(ret)) {
          return ret.unwrap().then(
            (payload) => payload,
            (reason: unknown) => {
              throw toCallError(reason)
            },
          )
        }
        return ret
      },
    },
    options,
  )
}

export type { ExposeOptions, ActionOptions, ResourceOptions, Dispose } from './index'
