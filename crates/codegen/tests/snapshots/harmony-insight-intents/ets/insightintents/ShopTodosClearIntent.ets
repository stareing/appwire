// 由 app-mcp-codegen 生成（target: harmony-insight-intents），请勿手动修改。
// App：示例商城（shop）
// 总览：演示用购物商城，可管理待办、浏览商品、操作购物车并结算
// 工具 `todos.clear` 的意图执行器（insight_intent.json 的 insightIntentsSrcEntry 引用本文件）。

import { insightIntent, InsightIntentEntry, InsightIntentEntryExecutor } from '@kit.AbilityKit';
import { ShopIntentRuntime } from '../appmcp/ShopInsightIntents';
import { ShopToolHandlers, TodosClearParams } from '../appmcp/ShopTools';

/**
 * 删除全部已完成的待办
 *
 * 工具 `todos.clear`「清空待办」，风险：destructive
 * 风险 destructive：意图框架没有系统级确认，以前台模式执行，handler 应在执行前向用户确认。
 */
@InsightIntentEntry({
  intentName: "ShopTodosClear",
  domain: "ToolsDomain",
  intentVersion: "1.0.0",
  displayName: "清空待办",
  displayDescription: "删除全部已完成的待办",
  llmDescription: "删除全部已完成的待办",
  keywords: ["清空待办", "todos.clear"],
  abilityName: "EntryAbility",
  executeMode: [insightIntent.ExecuteMode.UI_ABILITY_FOREGROUND],
  parameters: {
    "description": "删除全部已完成的待办",
    "properties": {},
    "type": "object"
  },
})
export default class ShopTodosClearIntent extends InsightIntentEntryExecutor<string> {
  onExecute(): Promise<insightIntent.IntentResult<string>> {
    const params: TodosClearParams = {};
    return ShopIntentRuntime.run((handlers: ShopToolHandlers) => handlers.todosClear(params));
  }
}
