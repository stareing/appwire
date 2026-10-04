/**
 * 工具 / 资源 `cache` 的格式校验（spec/protocol.md 3.6、spec/manifest.md 第 3 节，规则与 crates/manifest `validate_cache`
 * 及 crates/protocol `CachePolicy` 的解析一致）：`ttlMs` 须为整数 1..=86400000、`scope` 须为 `private` / `shared`（错误）；
 * 工具的生效注解不是只读时给出警告（Hub 忽略写工具上的声明）。
 */
import type { Risk, ToolAnnotations } from '@app-mcp/web'

/** `ttlMs` 上限：24 小时（crates/protocol `MAX_CACHE_TTL_MS`）。 */
export const MAX_CACHE_TTL_MS = 86_400_000
const CACHE_SCOPES: readonly unknown[] = ['private', 'shared']

/**
 * 校验 `cache` 的格式，问题以 `${label} cache…` 追加到 `errors`。
 * @input value 未经校验的字段值（清单 JSON 或构建期定义）。
 */
export function checkCache(value: unknown, label: string, errors: string[]): void {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) {
    errors.push(`${label} cache 必须是对象 {ttlMs, scope?}`)
    return
  }
  const { ttlMs, scope } = value as { ttlMs?: unknown; scope?: unknown }
  if (typeof ttlMs !== 'number' || !Number.isInteger(ttlMs) || ttlMs < 1 || ttlMs > MAX_CACHE_TTL_MS) {
    errors.push(`${label} cache.ttlMs 须为 1..=${MAX_CACHE_TTL_MS} 之间的整数（为 ${JSON.stringify(ttlMs)}）`)
  }
  if (scope !== undefined && !CACHE_SCOPES.includes(scope)) {
    errors.push(`${label} cache.scope ${JSON.stringify(scope)} 不合法，应为 private 或 shared`)
  }
}

/**
 * 工具声明了 `cache` 但生效注解不是只读时的警告（生效注解：声明的 `readOnlyHint` 优先，否则 `risk` 为 `read` 时只读，
 * 与 crates/protocol `ToolAnnotations::effective` 一致）；无需警告时返回 undefined。
 */
export function cacheReadOnlyWarning(
  tool: { risk?: Risk; annotations?: ToolAnnotations },
  label: string,
): string | undefined {
  const readOnly = tool.annotations?.readOnlyHint ?? tool.risk === 'read'
  return readOnly ? undefined : `${label} cache 只对只读工具（生效注解 readOnlyHint 为 true）生效，Hub 会忽略此声明`
}
