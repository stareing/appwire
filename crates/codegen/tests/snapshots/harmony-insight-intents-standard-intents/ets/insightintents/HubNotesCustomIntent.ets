// 由 app-mcp-codegen 生成（target: harmony-insight-intents），请勿手动修改。
// App：生活助手（hub）
// 简介：标准意图快照用例：六个试点动词各一个实现者，另含跳过规则
// 工具 `notes.custom` 的意图执行器（insight_intent.json 的 insightIntentsSrcEntry 引用本文件）。

import { insightIntent, InsightIntentEntry, InsightIntentEntryExecutor } from '@kit.AbilityKit';
import { HubIntentRuntime } from '../appmcp/HubInsightIntents';
import { HubToolHandlers, NotesCustomParams } from '../appmcp/HubTools';

/**
 * 词表外动词
 *
 * 工具 `notes.custom`，风险：write
 */
@InsightIntentEntry({
  intentName: "HubNotesCustom",
  domain: "ToolsDomain",
  intentVersion: "1.0.0",
  displayName: "notes.custom",
  displayDescription: "词表外动词",
  llmDescription: "词表外动词",
  keywords: ["notes.custom"],
  abilityName: "EntryAbility",
  executeMode: [insightIntent.ExecuteMode.UI_ABILITY_BACKGROUND],
  parameters: {
    "description": "词表外动词",
    "properties": {
      "body": {
        "type": "string"
      }
    },
    "type": "object"
  },
})
export default class HubNotesCustomIntent extends InsightIntentEntryExecutor<string> {
  public body?: string;

  onExecute(): Promise<insightIntent.IntentResult<string>> {
    const params: NotesCustomParams = {
      body: this.body,
    };
    return HubIntentRuntime.run((handlers: HubToolHandlers) => handlers.notesCustom(params));
  }
}
