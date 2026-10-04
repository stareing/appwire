// 由 app-mcp-codegen 生成（target: harmony-insight-intents），请勿手动修改。
// App：弃用示例（legacy）
// 简介：演示工具级与参数级弃用的生成结果
// 工具 `cart.legacyClear` 的意图执行器（insight_intent.json 的 insightIntentsSrcEntry 引用本文件）。

import { insightIntent, InsightIntentEntry, InsightIntentEntryExecutor } from '@kit.AbilityKit';
import { LegacyIntentRuntime } from '../appmcp/LegacyInsightIntents';
import { CartLegacyClearParams, LegacyToolHandlers } from '../appmcp/LegacyTools';

/**
 * 清空购物车（旧版）
 *
 * 工具 `cart.legacyClear`「清空购物车」，风险：write
 * 已弃用：改用 cart.clear
 */
@InsightIntentEntry({
  intentName: "LegacyCartLegacyClear",
  domain: "ToolsDomain",
  intentVersion: "1.0.0",
  displayName: "清空购物车",
  displayDescription: "清空购物车（旧版）",
  llmDescription: "清空购物车（旧版）",
  keywords: ["清空购物车", "cart.legacyClear"],
  abilityName: "EntryAbility",
  executeMode: [insightIntent.ExecuteMode.UI_ABILITY_BACKGROUND],
  parameters: {
    "description": "清空购物车（旧版）",
    "properties": {},
    "type": "object"
  },
})
export default class LegacyCartLegacyClearIntent extends InsightIntentEntryExecutor<string> {
  onExecute(): Promise<insightIntent.IntentResult<string>> {
    const params: CartLegacyClearParams = {};
    return LegacyIntentRuntime.run((handlers: LegacyToolHandlers) => handlers.cartLegacyClear(params));
  }
}
