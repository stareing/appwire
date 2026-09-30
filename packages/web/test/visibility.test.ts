import { describe, expect, it, vi } from 'vitest'
import { watchVisibility } from '../src/visibility'

describe('watchVisibility', () => {
  it('无 window / document 时（SSR）返回 visible', () => {
    const onChange = vi.fn()
    const w = watchVisibility(onChange, undefined, undefined)
    expect(w.current()).toEqual({ visibility: 'visible', focused: true })
    w.dispose()
  })

  it('只在变化时通知，dispose 后不再通知', () => {
    const doc = new EventTarget() as EventTarget & { visibilityState: string; hasFocus(): boolean }
    doc.visibilityState = 'visible'
    doc.hasFocus = () => true
    const win = new EventTarget()
    const onChange = vi.fn()
    const w = watchVisibility(onChange, win as Window, doc as unknown as Document)
    win.dispatchEvent(new Event('focus'))
    expect(onChange).not.toHaveBeenCalled()
    doc.dispatchEvent(new Event('freeze'))
    expect(onChange).toHaveBeenLastCalledWith({ visibility: 'frozen', focused: false })
    w.dispose()
    doc.dispatchEvent(new Event('resume'))
    expect(onChange).toHaveBeenCalledTimes(1)
  })
})
