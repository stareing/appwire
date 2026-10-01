// 由 app-mcp-codegen 生成（target: harmony-insight-intents），请勿手动修改。
// App：示例商城（shop）
// 总览：演示用购物商城，可管理待办、浏览商品、操作购物车并结算
// 工具 `cart.removeItem` 的意图执行器（insight_intent.json 的 insightIntentsSrcEntry 引用本文件）。

import { insightIntent, InsightIntentEntry, InsightIntentEntryExecutor } from '@kit.AbilityKit';
import { ShopIntentRuntime } from '../appmcp/ShopInsightIntents';
import { CartRemoveItemParams, ShopToolHandlers } from '../appmcp/ShopTools';

/**
 * 从购物车移除一个条目
 *
 * 工具 `cart.removeItem`「移除购物车条目」，风险：destructive
 * 风险 destructive：意图框架没有系统级确认，以前台模式执行，handler 应在执行前向用户确认。
 */
@InsightIntentEntry({
  intentName: "ShopCartRemoveItem",
  domain: "ToolsDomain",
  intentVersion: "1.0.0",
  displayName: "移除购物车条目",
  displayDescription: "从购物车移除一个条目",
  llmDescription: "从购物车移除一个条目",
  keywords: ["移除购物车条目", "cart.removeItem"],
  abilityName: "EntryAbility",
  executeMode: [insightIntent.ExecuteMode.UI_ABILITY_FOREGROUND],
  parameters: {
    "description": "从购物车移除一个条目",
    "properties": {
      "itemId": {
        "type": "string"
      }
    },
    "required": [
      "itemId"
    ],
    "type": "object"
  },
})
export default class ShopCartRemoveItemIntent extends InsightIntentEntryExecutor<string> {
  public itemId?: string;

  onExecute(): Promise<insightIntent.IntentResult<string>> {
    const itemIdValue = this.itemId;
    if (itemIdValue === undefined) {
      return ShopIntentRuntime.missing("itemId");
    }
    const params: CartRemoveItemParams = {
      itemId: itemIdValue,
    };
    return ShopIntentRuntime.run((handlers: ShopToolHandlers) => handlers.cartRemoveItem(params));
  }
}
