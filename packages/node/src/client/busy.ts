/**
 * 用户正在操作（spec/protocol.md 5.3）的有效值：显式开关（`setBusy`）OR 未结束的作用域（`beginBusy`）数 > 0。
 * 只在有效值变化时通知原生客户端。与 @app-mcp/web 的 `src/busy.ts` 同义（两包互不依赖，各自实现）。
 */

import type { BusyHandle } from '../types.js'

export class BusyState {
  private explicit = false
  private scopes = 0

  /** @input onChange 有效值变化时调用（参数为新值）。 */
  constructor(private readonly onChange: (busy: boolean) => void) {}

  /** 有效值。 */
  get busy(): boolean {
    return this.explicit || this.scopes > 0
  }

  /** 显式开关；不结束进行中的作用域。 */
  set(busy: boolean): void {
    this.update(() => {
      this.explicit = busy
    })
  }

  /** 开始一个作用域（可嵌套，引用计数）；返回的句柄 `release` 幂等，结束作用域不清除显式开关。 */
  begin(): BusyHandle {
    this.update(() => {
      this.scopes++
    })
    let released = false
    return {
      release: () => {
        if (released) return
        released = true
        this.update(() => {
          this.scopes--
        })
      },
    }
  }

  private update(mutate: () => void): void {
    const before = this.busy
    mutate()
    const after = this.busy
    if (after !== before) this.onChange(after)
  }
}
