// 由 app-mcp-codegen 生成（target: harmony-insight-intents），请勿手动修改。
// App：示例商城（shop）
// 总览：演示用购物商城，可管理待办、浏览商品、操作购物车并结算
// 工具 `orders.export` 的意图执行器（insight_intent.json 的 insightIntentsSrcEntry 引用本文件）。

import { insightIntent, InsightIntentEntry, InsightIntentEntryExecutor } from '@kit.AbilityKit';
import { ShopIntentRuntime } from '../appmcp/ShopInsightIntents';
import { OrdersExportParams, ShopToolHandlers } from '../appmcp/ShopTools';

/**
 * 按条件导出订单
 *
 * 工具 `orders.export`「导出订单」，风险：write
 */
@InsightIntentEntry({
  intentName: "ShopOrdersExport",
  domain: "ToolsDomain",
  intentVersion: "1.0.0",
  displayName: "导出订单",
  displayDescription: "按条件导出订单",
  llmDescription: "按条件导出订单",
  keywords: ["导出订单", "orders.export"],
  abilityName: "EntryAbility",
  executeMode: [insightIntent.ExecuteMode.UI_ABILITY_BACKGROUND],
  parameters: {
    "description": "按条件导出订单",
    "properties": {
      "extra": {
        "description": "透传给导出器的任意 JSON"
      },
      "filter": {
        "description": "筛选条件（任意形式）"
      },
      "labels": {
        "additionalProperties": {
          "type": "string"
        },
        "description": "附加标签",
        "type": "object"
      },
      "template": {}
    },
    "type": "object"
  },
})
export default class ShopOrdersExportIntent extends InsightIntentEntryExecutor<string> {
  /**
   * 筛选条件（任意形式）
   * 原始 JSON（不支持一般形式的 `oneOf`）
   */
  public filter?: Object | null;
  /** 附加标签 */
  public labels?: Record<string, string>;
  /** 透传给导出器的任意 JSON */
  public extra?: Object | null;
  /** 原始 JSON（不支持 `$ref`） */
  public template?: Object | null;

  onExecute(): Promise<insightIntent.IntentResult<string>> {
    const params: OrdersExportParams = {
      filter: this.filter,
      labels: this.labels,
      extra: this.extra,
      template: this.template,
    };
    return ShopIntentRuntime.run((handlers: ShopToolHandlers) => handlers.ordersExport(params));
  }
}
