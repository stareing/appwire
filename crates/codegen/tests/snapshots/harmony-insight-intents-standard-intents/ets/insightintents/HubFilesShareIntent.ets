// 由 app-mcp-codegen 生成（target: harmony-insight-intents），请勿手动修改。
// App：生活助手（hub）
// 简介：标准意图快照用例：六个试点动词各一个实现者，另含跳过规则
// 工具 `files.share` 的意图执行器（insight_intent.json 的 insightIntentsSrcEntry 引用本文件）。

import { insightIntent, InsightIntentEntry, InsightIntentEntryExecutor } from '@kit.AbilityKit';
import { HubIntentRuntime } from '../appmcp/HubInsightIntents';
import { FilesShareParams, HubToolHandlers } from '../appmcp/HubTools';

/**
 * 把文件分享给联系人
 *
 * 工具 `files.share`「分享文件」，风险：write
 */
@InsightIntentEntry({
  intentName: "HubFilesShare",
  domain: "ToolsDomain",
  intentVersion: "1.0.0",
  displayName: "分享文件",
  displayDescription: "把文件分享给联系人",
  llmDescription: "把文件分享给联系人",
  keywords: ["分享文件", "files.share"],
  abilityName: "EntryAbility",
  executeMode: [insightIntent.ExecuteMode.UI_ABILITY_BACKGROUND],
  parameters: {
    "description": "把文件分享给联系人",
    "properties": {
      "files": {
        "items": {
          "type": "string"
        },
        "minItems": 1,
        "type": "array"
      },
      "mimeType": {
        "type": "string"
      },
      "text": {
        "type": "string"
      }
    },
    "required": [
      "files"
    ],
    "type": "object"
  },
})
export default class HubFilesShareIntent extends InsightIntentEntryExecutor<string> {
  /** 元素个数：≥ 1 */
  public files?: string[];
  public mimeType?: string;
  public text?: string;

  onExecute(): Promise<insightIntent.IntentResult<string>> {
    const filesValue = this.files;
    if (filesValue === undefined) {
      return HubIntentRuntime.missing("files");
    }
    const params: FilesShareParams = {
      files: filesValue,
      mimeType: this.mimeType,
      text: this.text,
    };
    return HubIntentRuntime.run((handlers: HubToolHandlers) => handlers.filesShare(params));
  }
}
