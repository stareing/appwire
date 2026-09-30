/**
 * 元素引用：`eN`，在同一页面生命周期内稳定。
 *
 * 元素 → 引用用 WeakMap（不阻止回收）；引用 → 元素用 WeakRef。元素移除后引用失效。
 */

export const REF_PATTERN = /^e[1-9]\d*$/

export class RefRegistry {
  private byElement = new WeakMap<Element, string>()
  private byRef = new Map<string, WeakRef<Element>>()
  private next = 1

  /** 取得（必要时分配）元素的引用。 */
  refOf(el: Element): string {
    let ref = this.byElement.get(el)
    if (!ref) {
      ref = `e${this.next++}`
      this.byElement.set(el, ref)
      this.byRef.set(ref, new WeakRef(el))
    }
    return ref
  }

  /** 已分配的引用（不分配新引用）。 */
  peek(el: Element): string | undefined {
    return this.byElement.get(el)
  }

  /** 按引用查找元素；元素已被回收或已离开文档时返回 undefined。 */
  lookup(ref: string): Element | undefined {
    const el = this.byRef.get(ref)?.deref()
    if (!el) {
      this.byRef.delete(ref)
      return undefined
    }
    return el.isConnected ? el : undefined
  }

  /** 丢弃已被回收的元素对应的条目。 */
  prune(): void {
    for (const [ref, weak] of this.byRef) if (!weak.deref()) this.byRef.delete(ref)
  }

  clear(): void {
    this.byElement = new WeakMap()
    this.byRef.clear()
  }
}
