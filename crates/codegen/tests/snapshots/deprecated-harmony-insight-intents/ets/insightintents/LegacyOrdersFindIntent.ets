// 由 app-mcp-codegen 生成（target: harmony-insight-intents），请勿手动修改。
// App：弃用示例（legacy）
// 简介：演示工具级与参数级弃用的生成结果
// 工具 `orders.find` 的意图执行器（insight_intent.json 的 insightIntentsSrcEntry 引用本文件）。

import { insightIntent, InsightIntentEntity, InsightIntentEntry, InsightIntentEntryExecutor } from '@kit.AbilityKit';
import { LegacyIntentRuntime } from '../appmcp/LegacyInsightIntents';
import { LegacyToolHandlers, OrdersFindFilter, OrdersFindParams } from '../appmcp/LegacyTools';

/**
 * 过滤条件
 * 参数 `filter` 的意图实体（系统入口按 parameters 中该属性的 schema 赋值）。
 */
@InsightIntentEntity({
  entityCategory: "LegacyOrdersFind.filter",
})
export class LegacyOrdersFindFilterEntity implements insightIntent.IntentEntity {
  public entityId: string = '';
  /** 关键词 */
  public keyword?: string;
  /** 旧标签 */
  public legacyTag?: string;
}

/** LegacyOrdersFindFilterEntity → OrdersFindFilter。 */
function toOrdersFindFilter(entity: LegacyOrdersFindFilterEntity): OrdersFindFilter {
  const converted: OrdersFindFilter = {
    keyword: entity.keyword,
    legacyTag: entity.legacyTag,
  };
  return converted;
}

/**
 * 按状态查找订单
 *
 * 工具 `orders.find`「查找订单」，风险：read
 */
@InsightIntentEntry({
  intentName: "LegacyOrdersFind",
  domain: "ToolsDomain",
  intentVersion: "1.0.0",
  displayName: "查找订单",
  displayDescription: "按状态查找订单",
  llmDescription: "按状态查找订单",
  keywords: ["查找订单", "orders.find"],
  abilityName: "EntryAbility",
  executeMode: [insightIntent.ExecuteMode.UI_ABILITY_BACKGROUND],
  parameters: {
    "description": "按状态查找订单",
    "properties": {
      "cursor": {
        "description": "分页游标",
        "type": "string"
      },
      "filter": {
        "description": "过滤条件",
        "properties": {
          "keyword": {
            "description": "关键词",
            "type": "string"
          },
          "legacyTag": {
            "description": "旧标签",
            "type": "string"
          }
        },
        "type": "object"
      },
      "state": {
        "description": "订单状态",
        "type": "string"
      }
    },
    "required": [
      "state"
    ],
    "type": "object"
  },
})
export default class LegacyOrdersFindIntent extends InsightIntentEntryExecutor<string> {
  /** 订单状态 */
  public state?: string;
  /** 分页游标 */
  public cursor?: string;
  /** 过滤条件 */
  public filter?: LegacyOrdersFindFilterEntity;

  onExecute(): Promise<insightIntent.IntentResult<string>> {
    const stateValue = this.state;
    if (stateValue === undefined) {
      return LegacyIntentRuntime.missing("state");
    }
    const filterEntity = this.filter;
    const filterValue = filterEntity === undefined ? filterEntity : toOrdersFindFilter(filterEntity);
    const params: OrdersFindParams = {
      state: stateValue,
      cursor: this.cursor,
      filter: filterValue,
    };
    return LegacyIntentRuntime.run((handlers: LegacyToolHandlers) => handlers.ordersFind(params));
  }
}
