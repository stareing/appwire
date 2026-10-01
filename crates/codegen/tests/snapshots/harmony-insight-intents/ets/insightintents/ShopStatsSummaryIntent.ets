// 由 app-mcp-codegen 生成（target: harmony-insight-intents），请勿手动修改。
// App：示例商城（shop）
// 总览：演示用购物商城，可管理待办、浏览商品、操作购物车并结算
// 工具 `stats.summary` 的意图执行器（insight_intent.json 的 insightIntentsSrcEntry 引用本文件）。

import { insightIntent, InsightIntentEntry, InsightIntentEntryExecutor } from '@kit.AbilityKit';
import { ShopIntentRuntime } from '../appmcp/ShopInsightIntents';
import { ShopToolHandlers, StatsSummaryParams, StatsSummaryPeriod } from '../appmcp/ShopTools';

/**
 * 查看销售概览
 *
 * 工具 `stats.summary`「销售概览」，风险：read
 */
@InsightIntentEntry({
  intentName: "ShopStatsSummary",
  domain: "ToolsDomain",
  intentVersion: "1.0.0",
  displayName: "销售概览",
  displayDescription: "查看销售概览",
  llmDescription: "查看销售概览",
  keywords: ["销售概览", "stats.summary"],
  abilityName: "EntryAbility",
  executeMode: [insightIntent.ExecuteMode.UI_ABILITY_BACKGROUND],
  parameters: {
    "description": "查看销售概览",
    "properties": {
      "period": {
        "description": "统计周期",
        "enum": [
          "day",
          "week",
          "month"
        ],
        "type": "string"
      }
    },
    "type": "object"
  },
})
export default class ShopStatsSummaryIntent extends InsightIntentEntryExecutor<string> {
  /** 统计周期 */
  public period?: StatsSummaryPeriod | null;

  onExecute(): Promise<insightIntent.IntentResult<string>> {
    const params: StatsSummaryParams = {
      period: this.period,
    };
    return ShopIntentRuntime.run((handlers: ShopToolHandlers) => handlers.statsSummary(params));
  }
}
