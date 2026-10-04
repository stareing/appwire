/**
 * 工具 `deprecated` 的格式校验（spec/protocol.md 3.7、spec/manifest.md 第 3 节，规则与 crates/protocol `Deprecation::validate`
 * 及 crates/manifest `validate_deprecation` 一致）：`message` 为 1..=500 个字符的非空文本、`replacement` 为合法局部名且不指向
 * 自身、`until` 为合法的 `YYYY-MM-DD` 日期（错误）；`inputSchema` 中标了 `deprecated: true` 的必填参数给出警告。
 */
import { NAME_PATTERN } from './manifest'

/** `message` 的最大字符数（crates/protocol `MAX_DEPRECATION_MESSAGE_CHARS`）。 */
export const MAX_DEPRECATION_MESSAGE_CHARS = 500
const FULL_DATE = /^(\d{4})-(\d{2})-(\d{2})$/

/**
 * 校验 `deprecated` 的格式，问题以 `${label} deprecated…` 追加到 `errors`。
 * @input value 未经校验的字段值（清单 JSON 或构建期定义）；tool 为声明所在工具的局部名。
 */
export function checkDeprecation(value: unknown, tool: unknown, label: string, errors: string[]): void {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) {
    errors.push(`${label} deprecated 必须是对象 {message, replacement?, until?}`)
    return
  }
  const { message, replacement, until } = value as { message?: unknown; replacement?: unknown; until?: unknown }
  // @why 按 Unicode 码位计数，与 Rust `chars().count()` 一致（UTF-16 的 length 会把代理对算成 2）。
  const chars = typeof message === 'string' ? [...message].length : 0
  if (typeof message !== 'string' || message.trim() === '' || chars > MAX_DEPRECATION_MESSAGE_CHARS) {
    errors.push(`${label} deprecated.message 须为 1..=${MAX_DEPRECATION_MESSAGE_CHARS} 个字符的非空文本（为 ${chars} 个）`)
  }
  if (replacement !== undefined) {
    if (typeof replacement !== 'string' || !NAME_PATTERN.test(replacement)) {
      errors.push(`${label} deprecated.replacement ${JSON.stringify(replacement)} 不是合法的工具局部名`)
    } else if (replacement === tool) {
      errors.push(`${label} deprecated.replacement 不能指向工具自身`)
    }
  }
  if (until !== undefined && (typeof until !== 'string' || !isFullDate(until))) {
    errors.push(`${label} deprecated.until ${JSON.stringify(until)} 不是 RFC 3339 日期（YYYY-MM-DD）`)
  }
}

/**
 * `inputSchema` 顶层 `required` 中、对应属性标了 `deprecated: true` 的参数名（crates/protocol `deprecated_required_params`）：
 * 必填与弃用矛盾，只给出警告。schema 结构不合预期时视为无。
 */
export function deprecatedRequiredWarnings(inputSchema: unknown, label: string): string[] {
  if (inputSchema === null || typeof inputSchema !== 'object') return []
  const { properties, required } = inputSchema as { properties?: unknown; required?: unknown }
  if (!Array.isArray(required)) return []
  const isDeprecated = (name: string): boolean => {
    if (properties === null || typeof properties !== 'object') return false
    const prop = (properties as Record<string, unknown>)[name]
    return prop !== null && typeof prop === 'object' && (prop as { deprecated?: unknown }).deprecated === true
  }
  return required
    .filter((n): n is string => typeof n === 'string' && isDeprecated(n))
    .map((n) => `${label} 必填参数 "${n}" 标了 deprecated: true：必填与弃用矛盾，应改为可选或去掉弃用标记`)
}

/** RFC 3339 full-date：`YYYY-MM-DD`，月份与当月天数（含闰年）合法。 */
function isFullDate(text: string): boolean {
  const m = FULL_DATE.exec(text)
  if (!m) return false
  const [y, mo, d] = [Number(m[1]), Number(m[2]), Number(m[3])]
  if (mo < 1 || mo > 12) return false
  const leap = (y % 4 === 0 && y % 100 !== 0) || y % 400 === 0
  const days = [31, leap ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31][mo - 1] as number
  return d >= 1 && d <= days
}
