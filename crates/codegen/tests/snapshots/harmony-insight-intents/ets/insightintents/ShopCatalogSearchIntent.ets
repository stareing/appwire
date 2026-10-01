// 由 app-mcp-codegen 生成（target: harmony-insight-intents），请勿手动修改。
// App：示例商城（shop）
// 总览：演示用购物商城，可管理待办、浏览商品、操作购物车并结算
// 工具 `catalog.search` 的意图执行器（insight_intent.json 的 insightIntentsSrcEntry 引用本文件）。

import { insightIntent, InsightIntentEntry, InsightIntentEntryExecutor } from '@kit.AbilityKit';
import { ShopIntentRuntime } from '../appmcp/ShopInsightIntents';
import { CatalogSearchCategory, CatalogSearchParams, ShopToolHandlers } from '../appmcp/ShopTools';

/**
 * 按关键词搜索商品，返回商品 ID、名称、价格
 *
 * 工具 `catalog.search`「搜索商品」，风险：read
 */
@InsightIntentEntry({
  intentName: "ShopCatalogSearch",
  domain: "ToolsDomain",
  intentVersion: "1.0.0",
  displayName: "搜索商品",
  displayDescription: "按关键词搜索商品，返回商品 ID、名称、价格",
  llmDescription: "按关键词搜索商品，返回商品 ID、名称、价格",
  keywords: ["搜索商品", "catalog.search"],
  abilityName: "EntryAbility",
  executeMode: [insightIntent.ExecuteMode.UI_ABILITY_BACKGROUND],
  parameters: {
    "description": "按关键词搜索商品，返回商品 ID、名称、价格",
    "properties": {
      "category": {
        "description": "商品分类",
        "enum": [
          "electronics",
          "home-goods",
          "food"
        ],
        "type": "string"
      },
      "inStock": {
        "description": "只看有货",
        "type": "boolean"
      },
      "keyword": {
        "description": "商品名或分类，如“耳机”",
        "type": "string"
      },
      "limit": {
        "default": 20,
        "description": "最多返回条数",
        "maximum": 100,
        "minimum": 1,
        "type": "integer"
      },
      "maxPrice": {
        "description": "价格上限（元）",
        "exclusiveMinimum": 0,
        "type": "number"
      }
    },
    "type": "object"
  },
})
export default class ShopCatalogSearchIntent extends InsightIntentEntryExecutor<string> {
  /** 商品名或分类，如“耳机” */
  public keyword?: string;
  /** 商品分类 */
  public category?: CatalogSearchCategory;
  /**
   * 最多返回条数
   * 取值范围：≥ 1，≤ 100；默认值：20
   */
  public limit?: number;
  /** 只看有货 */
  public inStock?: boolean;
  /**
   * 价格上限（元）
   * 取值范围：> 0
   */
  public maxPrice?: number;

  onExecute(): Promise<insightIntent.IntentResult<string>> {
    const params: CatalogSearchParams = {
      keyword: this.keyword,
      category: this.category,
      limit: this.limit,
      inStock: this.inStock,
      maxPrice: this.maxPrice,
    };
    return ShopIntentRuntime.run((handlers: ShopToolHandlers) => handlers.catalogSearch(params));
  }
}
