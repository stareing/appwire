import { useEffect, useLayoutEffect } from 'react'

/** 浏览器中用 useLayoutEffect（提交后立即同步），服务端渲染时退化为 useEffect。 */
export const useIsomorphicLayoutEffect = typeof window === 'undefined' ? useEffect : useLayoutEffect

/**
 * 计算输入定义的比较键：
 * - 带 `toJSONSchema()` 的对象（zod v4 classic 等）按转换结果比较；
 * - 普通 JSON Schema 按 JSON 文本比较；
 * - 无法转换时返回 null（调用方按引用比较）。
 */
export function schemaKey(input: unknown): string | null {
  if (input === undefined) return ''
  try {
    if (input !== null && typeof input === 'object') {
      const maybe = input as { toJSONSchema?: unknown; _zod?: unknown }
      if (typeof maybe.toJSONSchema === 'function') {
        return JSON.stringify((maybe.toJSONSchema as () => unknown).call(input))
      }
      if ('_zod' in maybe) return null
    }
    return JSON.stringify(input) ?? null
  } catch {
    return null
  }
}

/** 按 Object.is 逐项比较依赖数组。 */
export function depsChanged(
  prev: readonly unknown[] | undefined,
  next: readonly unknown[] | undefined,
): boolean {
  if (prev === next) return false
  if (!prev || !next || prev.length !== next.length) return true
  for (let i = 0; i < prev.length; i++) {
    if (!Object.is(prev[i], next[i])) return true
  }
  return false
}
