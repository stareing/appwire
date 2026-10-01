/**
 * 执行一次调用：点击元素或提交表单，然后收集结果。
 *
 * 结果来源（先到先得）：
 * 1. 表单 submit 事件上的 `respondWith(promise)`（W3C WebMCP 声明式 API）；
 * 2. `data-mcp-result="事件名"`：元素上派发的 CustomEvent 的 `detail`；
 * 3. 以上都没有时，等待 settleMs 后结束，结果为 `settled`（没有结果；如何呈现见 result.ts）。
 * 任何时候元素上派发 `mcp:error` 都会让调用失败。
 */

import { ToolCallError } from '@app-mcp/web'
import { ATTR, toErrorKind } from './attrs'
import type { PageOutcome } from './result'

export const ERROR_EVENT = 'mcp:error'

export type Action =
  | { kind: 'click' }
  | {
      kind: 'form'
      form: HTMLFormElement
      /** true：调用 requestSubmit()；false：等待用户自己点提交（标准写法未设 toolautosubmit）。 */
      autosubmit: boolean
    }

export interface InvokeOptions {
  toolName: string
  element: Element
  action: Action
  resultEvent: string | undefined
  timeoutMs: number
  settleMs: number
  signal: AbortSignal
}

/** 本包在 submit 事件上附加的字段（与 WebMCP 的 SubmitEvent 扩展同名）。 */
export interface AgentSubmitEvent extends SubmitEvent {
  readonly agentInvoked: boolean
  respondWith(result: unknown): void
}

function toToolError(e: unknown): Error {
  if (e instanceof ToolCallError) return e
  if (typeof e === 'object' && e !== null && typeof (e as { kind?: unknown }).kind === 'string') {
    const x = e as { kind: string; message?: unknown; details?: Record<string, unknown> }
    return new ToolCallError(toErrorKind(x.kind), String(x.message ?? '调用失败'), x.details)
  }
  return new ToolCallError('HANDLER_ERROR', e instanceof Error ? e.message : String(e))
}

function customEvent(el: Element, type: string, detail: unknown): Event {
  const Ctor = el.ownerDocument.defaultView?.CustomEvent ?? CustomEvent
  return new Ctor(type, { bubbles: true, detail })
}

export function invoke(opts: InvokeOptions): Promise<PageOutcome> {
  const { element, action, resultEvent, signal, toolName } = opts
  return new Promise<PageOutcome>((resolve, reject) => {
    let done = false
    const cleanups: Array<() => void> = []
    const finish = (fn: () => void): void => {
      if (done) return
      done = true
      for (const c of cleanups.splice(0)) c()
      fn()
    }
    const ok = (value: unknown): void => finish(() => resolve({ kind: 'result', value }))
    const settled = (): void => finish(() => resolve({ kind: 'settled' }))
    const fail = (e: unknown): void => finish(() => reject(toToolError(e)))
    const listen = (target: EventTarget, type: string, fn: (e: Event) => void, capture = false): void => {
      target.addEventListener(type, fn, capture)
      cleanups.push(() => target.removeEventListener(type, fn, capture))
    }
    const later = (fn: () => void, ms: number): void => {
      const t = setTimeout(fn, ms)
      cleanups.push(() => clearTimeout(t))
    }

    if (signal.aborted) {
      reject(new ToolCallError('CANCELLED', '调用已取消'))
      return
    }
    listen(signal, 'abort', () => fail(new ToolCallError('CANCELLED', '调用已取消')))
    later(() => fail(new ToolCallError('TIMEOUT', `等待页面结果超时（${opts.timeoutMs} ms）`)), opts.timeoutMs)

    listen(element, ERROR_EVENT, (e) => {
      const d = ((e as CustomEvent).detail ?? {}) as { kind?: unknown; message?: unknown; details?: unknown }
      const details =
        typeof d.details === 'object' && d.details !== null ? (d.details as Record<string, unknown>) : undefined
      fail(new ToolCallError(toErrorKind(d.kind), String(d.message ?? '页面报告调用失败'), details))
    })
    if (resultEvent) listen(element, resultEvent, (e) => ok((e as CustomEvent).detail ?? null))

    let responded: Promise<unknown> | undefined
    /** 动作完成（点击后或表单提交事件派发完毕）后决定结果来源。 */
    const afterAction = (): void => {
      if (done) return
      if (responded) {
        responded.then(ok, fail)
        return
      }
      if (!resultEvent) later(settled, opts.settleMs)
    }

    if (action.kind === 'click') {
      try {
        ;(element as HTMLElement).click()
      } catch (e) {
        fail(e)
        return
      }
      afterAction()
      return
    }

    const { form, autosubmit } = action
    const doc = form.ownerDocument
    let submitted = false
    // 捕获阶段挂在 document 上：先于页面（包括 React 根节点）的监听器执行。
    listen(
      doc,
      'submit',
      (e) => {
        if (e.target !== form || submitted) return
        submitted = true
        Object.defineProperty(e, 'agentInvoked', { value: true, configurable: true })
        Object.defineProperty(e, 'respondWith', {
          configurable: true,
          value: (result: unknown) => {
            // 结果交给模型，不再做表单默认的页面跳转
            e.preventDefault()
            responded = Promise.resolve(result)
          },
        })
        // 手动提交由浏览器派发，页面监听器还没执行；等本次派发结束再取结果。
        if (!autosubmit) setTimeout(afterAction, 0)
      },
      true,
    )

    if (!autosubmit) {
      // 对应标准中的 :tool-form-active 与 toolactivated / toolcancel
      form.setAttribute(ATTR.active, '')
      cleanups.push(() => form.removeAttribute(ATTR.active))
      form.dispatchEvent(customEvent(form, 'toolactivated', { toolName }))
      listen(form, 'reset', () => fail(new ToolCallError('USER_REJECTED', '用户重置了表单，调用已取消')))
      cleanups.push(() => {
        if (!submitted) form.dispatchEvent(customEvent(form, 'toolcancel', { toolName }))
      })
      return
    }

    try {
      if (typeof form.requestSubmit === 'function') form.requestSubmit()
      else {
        const Ev = doc.defaultView?.Event ?? Event
        if (form.dispatchEvent(new Ev('submit', { bubbles: true, cancelable: true }))) form.submit()
      }
    } catch (e) {
      fail(e)
      return
    }
    if (!submitted && !done) {
      fail(new ToolCallError('INVALID_INPUT', '表单未提交（可能未通过校验）'))
      return
    }
    afterAction()
  })
}
