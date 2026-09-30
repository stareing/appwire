/**
 * App 总览（spec/protocol.md 第 7 节、spec/manifest.md 的 `overview` 字段）。
 *
 * 总览是给模型读的说明文字：Host 在会话中首次接触该 App 时附带。本模块不依赖 Node，
 * 运行时代码也可以使用（`@app-mcp/build/define`），保证清单与 `createAppMcp({ overview })` 一致。
 */
import type { AppOverview } from '@app-mcp/web'

export const OVERVIEW_SUMMARY_MAX = 100
export const OVERVIEW_BODY_MAX = 2000
/** 建议的正文小节。 */
export const OVERVIEW_SECTIONS = ['适用场景', '能力范围', '典型流程', '前置条件', '不支持的操作', '风险说明'] as const

/** 定义总览（恒等函数，只用于类型）。 */
export function defineOverview(overview: AppOverview): AppOverview {
  return overview
}

/** 按字符（Unicode 码点）计算长度，与“≤ 100 字符”的规则一致。 */
function charLength(text: string): number {
  return [...text].length
}

/**
 * 校验总览：`summary` 为空时报错；超长或正文缺少建议小节时给出警告。
 */
export function validateOverview(overview: unknown): { errors: string[]; warnings: string[] } {
  const errors: string[] = []
  const warnings: string[] = []
  if (overview === null || typeof overview !== 'object' || Array.isArray(overview)) {
    return { errors: ['overview 必须是对象'], warnings }
  }
  const { summary, body, locale } = overview as Record<string, unknown>
  if (typeof summary !== 'string' || summary.trim() === '') {
    errors.push('overview.summary 不能为空')
  } else if (charLength(summary) > OVERVIEW_SUMMARY_MAX) {
    warnings.push(`overview.summary 超过 ${OVERVIEW_SUMMARY_MAX} 字符（${charLength(summary)}），Host 会截断`)
  }
  if (body !== undefined) {
    if (typeof body !== 'string') {
      errors.push('overview.body 必须是字符串')
    } else {
      if (charLength(body) > OVERVIEW_BODY_MAX) {
        warnings.push(`overview.body 超过 ${OVERVIEW_BODY_MAX} 字符（${charLength(body)}），Host 会截断`)
      }
      const headings = body
        .split(/\r?\n/)
        .filter((line) => /^\s{0,3}#{1,6}\s/.test(line))
        .join('\n')
      if (body.trim() !== '' && !OVERVIEW_SECTIONS.some((s) => headings.includes(s))) {
        warnings.push(`overview.body 建议包含以下小节之一：${OVERVIEW_SECTIONS.join('、')}`)
      }
    }
  }
  if (locale !== undefined && (typeof locale !== 'string' || locale.trim() === '')) {
    errors.push('overview.locale 必须是非空字符串')
  }
  return { errors, warnings }
}
