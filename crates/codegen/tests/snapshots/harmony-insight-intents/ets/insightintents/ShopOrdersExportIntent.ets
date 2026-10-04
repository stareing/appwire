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
        "description": "透传给导出器的任意 JSON（JSON 文本）",
        "type": "string"
      },
      "filter": {
        "description": "筛选条件（任意形式）（JSON 文本）",
        "type": "string"
      },
      "labels": {
        "description": "附加标签（JSON 对象文本）",
        "type": "string"
      },
      "template": {
        "description": "JSON 文本",
        "type": "string"
      }
    },
    "type": "object"
  },
})
export default class ShopOrdersExportIntent extends InsightIntentEntryExecutor<string> {
  /**
   * 筛选条件（任意形式）
   * 原始 JSON（不支持一般形式的 `oneOf`）
   * 意图参数为 JSON 文本，执行时解析。
   */
  public filter?: string;
  /**
   * 附加标签
   * 意图参数为 JSON 文本，执行时解析。
   */
  public labels?: string;
  /**
   * 透传给导出器的任意 JSON
   * 意图参数为 JSON 文本，执行时解析。
   */
  public extra?: string;
  /**
   * 原始 JSON（不支持 `$ref`）
   * 意图参数为 JSON 文本，执行时解析。
   */
  public template?: string;

  onExecute(): Promise<insightIntent.IntentResult<string>> {
    const filterText = this.filter;
    if (typeof filterText === 'string' && !ShopIntentRuntime.isJson(filterText)) {
      return ShopIntentRuntime.invalid("filter", "须为 JSON 文本");
    }
    const labelsText = this.labels;
    if (typeof labelsText === 'string' && !ShopIntentRuntime.isJsonObject(labelsText, false)) {
      return ShopIntentRuntime.invalid("labels", "须为 JSON 对象文本");
    }
    const extraText = this.extra;
    if (typeof extraText === 'string' && !ShopIntentRuntime.isJson(extraText)) {
      return ShopIntentRuntime.invalid("extra", "须为 JSON 文本");
    }
    const templateText = this.template;
    if (typeof templateText === 'string' && !ShopIntentRuntime.isJson(templateText)) {
      return ShopIntentRuntime.invalid("template", "须为 JSON 文本");
    }
    const params: OrdersExportParams = {
      filter: filterText === undefined ? filterText : ShopIntentRuntime.parseJson(filterText),
      labels: labelsText === undefined ? labelsText : ShopIntentRuntime.parseJson(labelsText) as Record<string, string>,
      extra: extraText === undefined ? extraText : ShopIntentRuntime.parseJson(extraText),
      template: templateText === undefined ? templateText : ShopIntentRuntime.parseJson(templateText),
    };
    return ShopIntentRuntime.run((handlers: ShopToolHandlers) => handlers.ordersExport(params));
  }
}
