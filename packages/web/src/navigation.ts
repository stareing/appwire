/**
 * 执行一次导航请求（spec/protocol.md 3.4）：驱动层（WASM 核心）与桥接实现（Electron / Tauri 页面侧）共用。
 *
 * 顺序：没有回调 → `unsupported`；有打开的 {@link ViewLayer} 且 `whileLayerOpen` 为 `deny`（缺省）→ 拒绝；调用回调；
 * 等界面稳定（新页面的 `view` 工具注册，{@link settleView}）并立即重新评估门控；完成。
 */

import { toJsonValue } from './result'
import type { NavigationHandler, NavigationOptions, NavigationRequest } from './types'
import { openViewLayers, refreshViewTools, settleView } from './view'

export type NavigationKind = 'NAVIGATION_FAILED' | 'NAVIGATION_DENIED' | 'USER_ACTION_REQUIRED'

/** 导航结果；失败时 `details.reason` 见 spec/protocol.md 3.4。 */
export type NavigationResult =
  | { ok: true }
  | { ok: false; kind: NavigationKind; message: string; details: Record<string, unknown> }

const DEFAULT_SETTLE_MS = 500

export interface NavigationEnv {
  doc: Document | undefined
  win: Window | undefined
}

export function navigationFailure(kind: NavigationKind, message: string, reason: string): NavigationResult {
  return { ok: false, kind, message, details: { reason } }
}

/** 各类别缺省的 `details.reason`（`USER_ACTION_REQUIRED` 没有缺省：`reason` / `uri` 只在回调给出时出现）。 */
const DEFAULT_REASON: Readonly<Record<NavigationKind, string | undefined>> = {
  NAVIGATION_FAILED: 'error',
  NAVIGATION_DENIED: 'app',
  USER_ACTION_REQUIRED: undefined,
}

function isNavigationKind(kind: unknown): kind is NavigationKind {
  return typeof kind === 'string' && Object.prototype.hasOwnProperty.call(DEFAULT_REASON, kind)
}

/**
 * 回调抛出的错误 → 结果：`kind` 为 `NAVIGATION_DENIED` / `NAVIGATION_FAILED` / `USER_ACTION_REQUIRED` 的错误（按 `kind`
 * 字段识别，跨包的 ToolCallError 同样适用）保留，缺省 `reason` 见 {@link DEFAULT_REASON}；其他错误归为
 * `NAVIGATION_FAILED`（`error`）。
 */
export function navigationErrorResult(error: unknown): NavigationResult {
  const e = (typeof error === 'object' && error !== null ? error : {}) as { kind?: unknown; message?: unknown; details?: unknown }
  const message = typeof e.message === 'string' && e.message !== '' ? e.message : String(error ?? '导航回调出错')
  if (!isNavigationKind(e.kind)) return navigationFailure('NAVIGATION_FAILED', message, 'error')
  let details: Record<string, unknown> = {}
  try {
    const json = e.details === undefined ? undefined : toJsonValue(e.details)
    if (typeof json === 'object' && json !== null && !Array.isArray(json)) details = json as Record<string, unknown>
  } catch {
    details = {}
  }
  const reason = DEFAULT_REASON[e.kind]
  return { ok: false, kind: e.kind, message, details: reason === undefined ? details : { reason, ...details } }
}

/** 导航参数：只接受对象，其余（null、数组）视为没有参数。 */
export function navigationParams(params: unknown): Record<string, unknown> | undefined {
  return typeof params === 'object' && params !== null && !Array.isArray(params) ? (params as Record<string, unknown>) : undefined
}

export async function runNavigation(
  handler: NavigationHandler | null,
  request: NavigationRequest,
  options: NavigationOptions,
  env: NavigationEnv,
): Promise<NavigationResult> {
  const { page } = request
  if (!handler) return navigationFailure('NAVIGATION_FAILED', `App 没有设置导航回调，无法切换到页面「${page}」`, 'unsupported')
  const layers = openViewLayers(env.doc)
  if (layers.length > 0 && (options.whileLayerOpen ?? 'deny') === 'deny') {
    const names = layers.map((l) => `「${l.name}」`).join('、')
    return navigationFailure(
      'NAVIGATION_DENIED',
      `界面上有打开的弹层${names}，用户可能正在操作，未切换到页面「${page}」。请让用户先关闭弹层后重试。`,
      'app',
    )
  }
  try {
    await handler(request)
  } catch (error) {
    return navigationErrorResult(error)
  }
  await settleView(env.win, options.settleMs ?? DEFAULT_SETTLE_MS)
  if (env.doc) refreshViewTools(env.doc)
  return { ok: true }
}
