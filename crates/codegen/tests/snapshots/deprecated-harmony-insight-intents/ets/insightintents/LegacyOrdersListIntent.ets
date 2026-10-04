// 由 app-mcp-codegen 生成（target: harmony-insight-intents），请勿手动修改。
// App：弃用示例（legacy）
// 简介：演示工具级与参数级弃用的生成结果
// 工具 `orders.list` 的意图执行器（insight_intent.json 的 insightIntentsSrcEntry 引用本文件）。

import { insightIntent, InsightIntentEntry, InsightIntentEntryExecutor } from '@kit.AbilityKit';
import { LegacyIntentRuntime } from '../appmcp/LegacyInsightIntents';
import { LegacyToolHandlers, OrdersListParams } from '../appmcp/LegacyTools';

/**
 * 按状态列出订单（旧版）
 *
 * 工具 `orders.list`「列出订单」，风险：read
 * 已弃用：旧版 "列表" 接口 *\/ 不再维护：$x ${y} \(z) 'q' <b>&amp; #{w}
 * 请改用分页更好的新版（改用工具 orders.find；计划于 2027-06-30 移除）
 */
@InsightIntentEntry({
  intentName: "LegacyOrdersList",
  domain: "ToolsDomain",
  intentVersion: "1.0.0",
  displayName: "列出订单",
  displayDescription: "按状态列出订单（旧版）",
  llmDescription: "按状态列出订单（旧版）",
  keywords: ["列出订单", "orders.list"],
  abilityName: "EntryAbility",
  executeMode: [insightIntent.ExecuteMode.UI_ABILITY_BACKGROUND],
  parameters: {
    "description": "按状态列出订单（旧版）",
    "properties": {
      "limit": {
        "description": "最多返回条数（整数）",
        "type": "number"
      },
      "page": {
        "description": "页码（整数）",
        "minimum": 1,
        "type": "number"
      },
      "status": {
        "description": "订单状态",
        "type": "string"
      }
    },
    "required": [
      "status"
    ],
    "type": "object"
  },
})
export default class LegacyOrdersListIntent extends InsightIntentEntryExecutor<string> {
  /**
   * 订单状态
   * 参数已弃用（inputSchema 中 deprecated: true）
   */
  public status?: string;
  /**
   * 页码
   * 取值范围：≥ 1
   * 参数已弃用（inputSchema 中 deprecated: true）
   */
  public page?: number;
  /** 最多返回条数 */
  public limit?: number;

  onExecute(): Promise<insightIntent.IntentResult<string>> {
    const statusValue = this.status;
    if (statusValue === undefined) {
      return LegacyIntentRuntime.missing("status");
    }
    const pageValue = this.page;
    if (typeof pageValue === 'number' && !Number.isInteger(pageValue)) {
      return LegacyIntentRuntime.invalid("page", "须为整数");
    }
    const limitValue = this.limit;
    if (typeof limitValue === 'number' && !Number.isInteger(limitValue)) {
      return LegacyIntentRuntime.invalid("limit", "须为整数");
    }
    const params: OrdersListParams = {
      status: statusValue,
      page: pageValue,
      limit: limitValue,
    };
    return LegacyIntentRuntime.run((handlers: LegacyToolHandlers) => handlers.ordersList(params));
  }
}
