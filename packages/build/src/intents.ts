/**
 * 工具 `implements` 的格式校验（spec/intents.md 第 1 节，规则与 crates/protocol `intents::implements_errors` 一致）：
 * 格式 / 重复 / 超过 4 项为错误。动词是否在词表中、是否满足词表必填参数（警告）由 `app-mcp-host validate`（crates/manifest）检查，
 * 这里不复制词表。
 */

/** 每个工具最多声明的意图数（crates/protocol `MAX_IMPLEMENTS`）。 */
export const MAX_IMPLEMENTS = 4
/** 动词名最大长度（crates/protocol `MAX_VERB_LEN`）。 */
const MAX_VERB_LEN = 64
const MAX_VERSION = 0xffff_ffff
/** `<域>.<动作>`：每段以字母开头，其后为字母、数字、`_`、`-`。 */
const VERB_PATTERN = /^[A-Za-z][A-Za-z0-9_-]*\.[A-Za-z][A-Za-z0-9_-]*$/
/** 主版本：无前导零的正整数。 */
const VERSION_PATTERN = /^[1-9][0-9]*$/

/** `"<动词>@<主版本>"` 是否合法。 */
export function isIntentId(text: string): boolean {
  const at = text.indexOf('@')
  if (at < 0) return false
  const verb = text.slice(0, at)
  const version = text.slice(at + 1)
  return (
    verb.length <= MAX_VERB_LEN &&
    VERB_PATTERN.test(verb) &&
    VERSION_PATTERN.test(version) &&
    Number(version) <= MAX_VERSION
  )
}

/**
 * 校验工具的 `implements`，问题以 `${label} implements…` 追加到 `errors`。
 * @input value 未经校验的字段值（清单 JSON 或构建期定义）。
 */
export function checkImplements(value: unknown, label: string, errors: string[]): void {
  if (!Array.isArray(value)) {
    errors.push(`${label} implements 必须是字符串数组`)
    return
  }
  if (value.length > MAX_IMPLEMENTS) errors.push(`${label} implements 最多 ${MAX_IMPLEMENTS} 项（实际 ${value.length} 项）`)
  value.forEach((item: unknown, i) => {
    const at = `${label} implements[${i}]`
    if (typeof item !== 'string' || !isIntentId(item)) {
      errors.push(
        `${at} 标准意图 ${JSON.stringify(item)} 格式不合法，应为 "<域>.<动作>@<主版本>"（如 "message.send@1"，主版本为正整数，spec/intents.md）`,
      )
    } else if (value.indexOf(item) < i) {
      errors.push(`${at} "${item}" 重复`)
    }
  })
}
