// 由 app-mcp-codegen 生成（target: harmony-insight-intents），请勿手动修改。
// App：生活助手（hub）
// 简介：标准意图快照用例：六个试点动词各一个实现者，另含跳过规则
// 工具 `notes.plain` 的意图执行器（insight_intent.json 的 insightIntentsSrcEntry 引用本文件）。

import { insightIntent, InsightIntentEntry, InsightIntentEntryExecutor } from '@kit.AbilityKit';
import { HubIntentRuntime } from '../appmcp/HubInsightIntents';
import { HubToolHandlers, NotesPlainParams } from '../appmcp/HubTools';

/**
 * 未声明意图
 *
 * 工具 `notes.plain`，风险：read
 */
@InsightIntentEntry({
  intentName: "HubNotesPlain",
  domain: "ToolsDomain",
  intentVersion: "1.0.0",
  displayName: "notes.plain",
  displayDescription: "未声明意图",
  llmDescription: "未声明意图",
  keywords: ["notes.plain"],
  abilityName: "EntryAbility",
  executeMode: [insightIntent.ExecuteMode.UI_ABILITY_BACKGROUND],
  parameters: {
    "description": "未声明意图",
    "properties": {},
    "type": "object"
  },
})
export default class HubNotesPlainIntent extends InsightIntentEntryExecutor<string> {
  onExecute(): Promise<insightIntent.IntentResult<string>> {
    const params: NotesPlainParams = {};
    return HubIntentRuntime.run((handlers: HubToolHandlers) => handlers.notesPlain(params));
  }
}
