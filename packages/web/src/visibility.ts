/**
 * 页面可见性与焦点跟踪。
 *
 * - `visibilitychange` → visible / hidden
 * - Page Lifecycle `freeze` / `resume` → frozen
 * - window `focus` / `blur` → focused
 */

import type { Visibility } from './types'

export interface VisibilitySnapshot {
  visibility: Visibility
  focused: boolean
}

export interface VisibilityWatcher {
  current(): VisibilitySnapshot
  dispose(): void
}

export function watchVisibility(
  onChange: (snapshot: VisibilitySnapshot) => void,
  win: Window | undefined = typeof window === 'undefined' ? undefined : window,
  doc: Document | undefined = typeof document === 'undefined' ? undefined : document,
): VisibilityWatcher {
  let frozen = false

  const read = (): VisibilitySnapshot => {
    const hidden = doc?.visibilityState === 'hidden'
    let focused = false
    try {
      focused = !hidden && !frozen && (typeof doc?.hasFocus === 'function' ? doc.hasFocus() : !hidden)
    } catch {
      focused = false
    }
    return { visibility: frozen ? 'frozen' : hidden ? 'hidden' : 'visible', focused }
  }

  let last = read()
  const update = (): void => {
    const next = read()
    if (next.visibility === last.visibility && next.focused === last.focused) return
    last = next
    onChange(next)
  }
  const onFreeze = (): void => {
    frozen = true
    update()
  }
  const onResume = (): void => {
    frozen = false
    update()
  }
  const onFocus = (): void => update()
  const onBlur = (): void => {
    // blur 时 hasFocus() 可能尚未更新，直接视为失焦
    const next: VisibilitySnapshot = { visibility: read().visibility, focused: false }
    if (next.visibility === last.visibility && next.focused === last.focused) return
    last = next
    onChange(next)
  }

  doc?.addEventListener('visibilitychange', update)
  doc?.addEventListener('freeze', onFreeze)
  doc?.addEventListener('resume', onResume)
  win?.addEventListener('focus', onFocus)
  win?.addEventListener('blur', onBlur)

  return {
    current: () => last,
    dispose() {
      doc?.removeEventListener('visibilitychange', update)
      doc?.removeEventListener('freeze', onFreeze)
      doc?.removeEventListener('resume', onResume)
      win?.removeEventListener('focus', onFocus)
      win?.removeEventListener('blur', onBlur)
    },
  }
}
