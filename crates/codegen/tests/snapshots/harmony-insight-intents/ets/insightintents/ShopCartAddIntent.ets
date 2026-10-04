// 由 app-mcp-codegen 生成（target: harmony-insight-intents），请勿手动修改。
// App：示例商城（shop）
// 总览：演示用购物商城，可管理待办、浏览商品、操作购物车并结算
// 工具 `cart.add` 的意图执行器（insight_intent.json 的 insightIntentsSrcEntry 引用本文件）。

import { insightIntent, InsightIntentEntry, InsightIntentEntryExecutor } from '@kit.AbilityKit';
import { ShopIntentRuntime } from '../appmcp/ShopInsightIntents';
import { CartAddParams, ShopToolHandlers } from '../appmcp/ShopTools';

/**
 * 把商品加入购物车
 *
 * 工具 `cart.add`「加入购物车」，风险：write
 */
@InsightIntentEntry({
  intentName: "ShopCartAdd",
  domain: "ToolsDomain",
  intentVersion: "1.0.0",
  displayName: "加入购物车",
  displayDescription: "把商品加入购物车",
  llmDescription: "把商品加入购物车",
  keywords: ["加入购物车", "cart.add"],
  abilityName: "EntryAbility",
  executeMode: [insightIntent.ExecuteMode.UI_ABILITY_BACKGROUND],
  parameters: {
    "description": "把商品加入购物车",
    "properties": {
      "note": {
        "description": "备注，可为 null",
        "type": "string"
      },
      "productId": {
        "description": "商品 ID",
        "type": "string"
      },
      "qty": {
        "description": "数量（整数）",
        "minimum": 1,
        "type": "number"
      }
    },
    "required": [
      "productId",
      "qty"
    ],
    "type": "object"
  },
})
export default class ShopCartAddIntent extends InsightIntentEntryExecutor<string> {
  /** 商品 ID */
  public productId?: string;
  /**
   * 数量
   * 取值范围：≥ 1
   */
  public qty?: number;
  /** 备注，可为 null */
  public note?: string | null;

  onExecute(): Promise<insightIntent.IntentResult<string>> {
    const productIdValue = this.productId;
    if (productIdValue === undefined) {
      return ShopIntentRuntime.missing("productId");
    }
    const qtyValue = this.qty;
    if (qtyValue === undefined) {
      return ShopIntentRuntime.missing("qty");
    }
    if (typeof qtyValue === 'number' && !Number.isInteger(qtyValue)) {
      return ShopIntentRuntime.invalid("qty", "须为整数");
    }
    const params: CartAddParams = {
      productId: productIdValue,
      qty: qtyValue,
      note: this.note,
    };
    return ShopIntentRuntime.run((handlers: ShopToolHandlers) => handlers.cartAdd(params));
  }
}
