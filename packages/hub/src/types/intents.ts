/** @app-mcp/hub 的公开类型：标准意图的机主默认表（spec/intents.md 第 4 节）。 */

/**
 * 机主默认表：键为动词（`message.send`，对其所有版本生效）或 `动词@主版本`（优先于不带版本的键），值为工具全名
 * （`<appId>.<tool>`）。最多 256 条、动词名最长 64 字符。默认只是提示：`apps.intents` 把它排在首位并标 `default: true`，
 * Hub 不按它路由。
 */
export type IntentDefaults = Record<string, string>

/** 意图默认表状态（`Hub.intents()`、`HubStatus.intents`）。 */
export interface IntentsStatus {
  defaults: IntentDefaults
  /** 最近一次替换失败的原因（之前的默认表继续生效）；之后成功时清除。 */
  lastError?: string
}
