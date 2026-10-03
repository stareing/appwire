/**
 * 页面声明的事件（spec/protocol.md 3.5，桥接 op `event.declare` / `event.remove` / `event.emit`）：按页面记录，转给主进程的
 * @app-mcp/node 客户端。页面刷新、卸载或 webContents 销毁时撤销该页的声明；多个页面声明同名事件时，最后一个页面撤销才
 * 撤销客户端上的声明（其余页面仍声明时以最近一次声明为准重新声明）。
 *
 * @why 主进程代码也可直接 `appMcp.declareEvent` 同名事件：页面全部撤销时会一并撤销它（同名即同一事件，客户端只有一份声明）。
 */

import type { AppMcp, EventDefinition, EventPayload } from '@app-mcp/node'
import type { EventMessage } from './protocol.js'

export type EventClient = Partial<Pick<AppMcp, 'declareEvent' | 'removeEvent' | 'emitEvent'>>

/** 页面键（webContents ID）。 */
type PageKey = number

export class PageEvents {
  /** 事件名 → 声明了它的页面（按声明先后，最近的在末尾）与各自的声明。 */
  private readonly byName = new Map<string, Map<PageKey, EventDefinition>>()

  constructor(private readonly appMcp: EventClient) {}

  /** @error 客户端不支持事件 → `code` 为 `UNSUPPORTED`；名称不合法 → 客户端的错误（`INVALID_NAME`）。 */
  declare(page: PageKey, event: EventMessage): void {
    const client = this.client()
    const definition: EventDefinition = {
      name: event.name,
      description: event.description,
      ...(event.payloadSchema !== undefined && { payloadSchema: event.payloadSchema }),
    }
    client.declareEvent(definition)
    const pages = this.byName.get(event.name) ?? new Map<PageKey, EventDefinition>()
    pages.delete(page)
    pages.set(page, definition)
    this.byName.set(event.name, pages)
  }

  /** 撤销本页的声明；返回本页是否声明过。 */
  remove(page: PageKey, name: string): boolean {
    const pages = this.byName.get(name)
    if (!pages?.delete(page)) return false
    if (pages.size === 0) {
      this.byName.delete(name)
      this.appMcp.removeEvent?.(name)
    } else {
      const latest = [...pages.values()].pop()
      if (latest) this.appMcp.declareEvent?.(latest)
    }
    return true
  }

  /** 页面结束：撤销其全部声明。 */
  release(page: PageKey): void {
    for (const [name, pages] of [...this.byName]) if (pages.has(page)) this.remove(page, name)
  }

  /** @error 同 @app-mcp/node 的 `emitEvent`；客户端不支持事件 → `UNSUPPORTED`。 */
  emit(name: string, payload: EventPayload | undefined): boolean {
    return this.client().emitEvent(name, payload)
  }

  private client(): Required<EventClient> {
    const { declareEvent, removeEvent, emitEvent } = this.appMcp
    if (!declareEvent || !removeEvent || !emitEvent) {
      throw Object.assign(new Error('appMcp 不支持事件（@app-mcp/node 版本过旧）'), { code: 'UNSUPPORTED' })
    }
    return this.appMcp as Required<EventClient>
  }
}
