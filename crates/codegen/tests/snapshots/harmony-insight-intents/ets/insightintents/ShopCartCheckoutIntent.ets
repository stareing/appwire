// 由 app-mcp-codegen 生成（target: harmony-insight-intents），请勿手动修改。
// App：示例商城（shop）
// 总览：演示用购物商城，可管理待办、浏览商品、操作购物车并结算
// 工具 `cart.checkout` 的意图执行器（insight_intent.json 的 insightIntentsSrcEntry 引用本文件）。

import { insightIntent, InsightIntentEntry, InsightIntentEntryExecutor } from '@kit.AbilityKit';
import { ShopIntentRuntime } from '../appmcp/ShopInsightIntents';
import { CartCheckoutItemsItem, CartCheckoutParams, CartCheckoutShipping, ShopToolHandlers } from '../appmcp/ShopTools';

/**
 * 提交订单并支付
 *
 * 工具 `cart.checkout`「结算」，风险：payment
 * 风险 payment：意图框架没有系统级确认，以前台模式执行，handler 应在执行前向用户确认。
 */
@InsightIntentEntry({
  intentName: "ShopCartCheckout",
  domain: "ToolsDomain",
  intentVersion: "1.0.0",
  displayName: "结算",
  displayDescription: "提交订单并支付",
  llmDescription: "提交订单并支付",
  keywords: ["结算", "cart.checkout"],
  abilityName: "EntryAbility",
  executeMode: [insightIntent.ExecuteMode.UI_ABILITY_FOREGROUND],
  parameters: {
    "description": "提交订单并支付",
    "properties": {
      "addressId": {
        "description": "收货地址 ID",
        "type": "string"
      },
      "coupon": {
        "description": "优惠码",
        "type": "string"
      },
      "giftWrap": {
        "default": false,
        "type": "boolean"
      },
      "items": {
        "description": "只结算这些条目；省略时结算全部",
        "items": {
          "properties": {
            "itemId": {
              "type": "string"
            },
            "qty": {
              "minimum": 1,
              "type": "integer"
            }
          },
          "required": [
            "itemId"
          ],
          "type": "object"
        },
        "minItems": 1,
        "type": "array"
      },
      "shipping": {
        "description": "配送选项",
        "properties": {
          "deliverAfter": {
            "description": "最早送达时间",
            "type": "string"
          },
          "method": {
            "description": "配送方式",
            "enum": [
              "standard",
              "express"
            ],
            "type": "string"
          }
        },
        "required": [
          "method"
        ],
        "type": "object"
      }
    },
    "required": [
      "addressId"
    ],
    "type": "object"
  },
})
export default class ShopCartCheckoutIntent extends InsightIntentEntryExecutor<string> {
  /** 收货地址 ID */
  public addressId?: string;
  /** 优惠码 */
  public coupon?: string;
  /** 配送选项 */
  public shipping?: CartCheckoutShipping;
  /**
   * 只结算这些条目；省略时结算全部
   * 元素个数：≥ 1
   */
  public items?: CartCheckoutItemsItem[];
  /** 默认值：false */
  public giftWrap?: boolean;

  onExecute(): Promise<insightIntent.IntentResult<string>> {
    const addressIdValue = this.addressId;
    if (addressIdValue === undefined) {
      return ShopIntentRuntime.missing("addressId");
    }
    const params: CartCheckoutParams = {
      addressId: addressIdValue,
      coupon: this.coupon,
      shipping: this.shipping,
      items: this.items,
      giftWrap: this.giftWrap,
    };
    return ShopIntentRuntime.run((handlers: ShopToolHandlers) => handlers.cartCheckout(params));
  }
}
