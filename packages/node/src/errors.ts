import type { ErrorKind } from './types.js'

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

export function isErrorKind(value: unknown): value is ErrorKind {
  return typeof value === 'string' && ERROR_KINDS.has(value)
}

/** handler 可以抛出此错误以指定错误类别；其他异常归为 HANDLER_ERROR。与 @app-mcp/web 同形。 */
export class ToolCallError extends Error {
  readonly kind: ErrorKind
  readonly details?: Record<string, unknown>
  constructor(kind: ErrorKind, message: string, details?: Record<string, unknown>) {
    super(message)
    this.name = 'ToolCallError'
    this.kind = kind
    this.details = details
  }
}

/**
 * 把 handler 抛出的异常转换为 `{ kind, message, details? }`（`details` 来自 ToolCallError，随错误的 `data` 发给 Host）。
 *
 * 按结构识别 ToolCallError（`name === 'ToolCallError'` 且 `kind` 为合法类别），
 * 因此 @app-mcp/web 或其他副本中的 ToolCallError 同样有效。
 */
export function toFailure(error: unknown): { kind: ErrorKind; message: string; details?: Record<string, unknown> } {
  if (typeof error === 'object' && error !== null) {
    const e = error as { name?: unknown; kind?: unknown; message?: unknown; details?: unknown }
    const message = typeof e.message === 'string' ? e.message : String(error)
    if ((error instanceof ToolCallError || e.name === 'ToolCallError') && isErrorKind(e.kind)) {
      const details = e.details
      return typeof details === 'object' && details !== null && !Array.isArray(details)
        ? { kind: e.kind, message, details: details as Record<string, unknown> }
        : { kind: e.kind, message }
    }
    return { kind: 'HANDLER_ERROR', message: message || 'handler 执行失败' }
  }
  return { kind: 'HANDLER_ERROR', message: error === undefined ? 'handler 执行失败' : String(error) }
}

/** 原生模块抛出的错误带 `code`（如 `DUPLICATE_NAME`、`ALREADY_COMPLETED`）。 */
export function nativeErrorCode(error: unknown): string | undefined {
  if (typeof error === 'object' && error !== null) {
    const code = (error as { code?: unknown }).code
    if (typeof code === 'string') return code
  }
  return undefined
}
