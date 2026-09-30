import type { BindingErrorCode, ErrorKind } from './types.js'

/**
 * Hub 操作失败。`kind` 为协议错误类别（如 `TOOL_NOT_FOUND`）或绑定层代码（`INVALID_ARG` / `SHUTDOWN` …）。
 * 注意：工具层面的失败（用户拒绝、超时、App 报错）不抛错，而在 `CallOutcome.result.error` 中。
 */
export class HubError extends Error {
  readonly kind: ErrorKind | BindingErrorCode
  /** 与 `kind` 相同，便于按 Node 习惯 `err.code` 判断。 */
  readonly code: ErrorKind | BindingErrorCode
  readonly details?: unknown

  constructor(kind: ErrorKind | BindingErrorCode, message: string, details?: unknown) {
    super(message)
    this.name = 'HubError'
    this.kind = kind
    this.code = kind
    this.details = details
  }
}

const CODE_PREFIX = /^\[([A-Z_]+)\] ?([\s\S]*)$/

/** 把原生模块抛出的错误（消息形如 `[CODE] 说明`）转为 {@link HubError}；其他错误原样返回。 */
export function fromNativeError(e: unknown): unknown {
  if (e instanceof HubError) return e
  if (e instanceof Error) {
    const m = CODE_PREFIX.exec(e.message)
    if (m) {
      const err = new HubError(m[1] as ErrorKind | BindingErrorCode, m[2] ?? '')
      err.cause = e
      return err
    }
  }
  return e
}
