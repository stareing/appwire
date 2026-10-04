// 由 app-mcp-codegen 生成（target: harmony-insight-intents），请勿手动修改。
// App：生活助手（hub）
// 简介：标准意图快照用例：六个试点动词各一个实现者，另含跳过规则
// 工具 `message.quick` 的意图执行器（insight_intent.json 的 insightIntentsSrcEntry 引用本文件）。

import { insightIntent, InsightIntentEntry, InsightIntentEntryExecutor } from '@kit.AbilityKit';
import { HubIntentRuntime } from '../appmcp/HubInsightIntents';
import { HubToolHandlers, MessageQuickParams } from '../appmcp/HubTools';

/**
 * 快捷回复（与 message.compose 声明同一动词，系统意图只绑定第一个）
 *
 * 工具 `message.quick`，风险：write
 */
@InsightIntentEntry({
  intentName: "HubMessageQuick",
  domain: "ToolsDomain",
  intentVersion: "1.0.0",
  displayName: "message.quick",
  displayDescription: "快捷回复（与 message.compose 声明同一动词，系统意图只绑定第一个）",
  llmDescription: "快捷回复（与 message.compose 声明同一动词，系统意图只绑定第一个）",
  keywords: ["message.quick"],
  abilityName: "EntryAbility",
  executeMode: [insightIntent.ExecuteMode.UI_ABILITY_BACKGROUND],
  parameters: {
    "description": "快捷回复（与 message.compose 声明同一动词，系统意图只绑定第一个）",
    "properties": {
      "text": {
        "type": "string"
      },
      "to": {
        "items": {
          "type": "string"
        },
        "type": "array"
      }
    },
    "required": [
      "to",
      "text"
    ],
    "type": "object"
  },
})
export default class HubMessageQuickIntent extends InsightIntentEntryExecutor<string> {
  public to?: string[];
  public text?: string;

  onExecute(): Promise<insightIntent.IntentResult<string>> {
    const toValue = this.to;
    if (toValue === undefined) {
      return HubIntentRuntime.missing("to");
    }
    const textValue = this.text;
    if (textValue === undefined) {
      return HubIntentRuntime.missing("text");
    }
    const params: MessageQuickParams = {
      to: toValue,
      text: textValue,
    };
    return HubIntentRuntime.run((handlers: HubToolHandlers) => handlers.messageQuick(params));
  }
}
