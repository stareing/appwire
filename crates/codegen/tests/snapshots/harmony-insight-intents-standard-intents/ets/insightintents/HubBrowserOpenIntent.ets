// 由 app-mcp-codegen 生成（target: harmony-insight-intents），请勿手动修改。
// App：生活助手（hub）
// 简介：标准意图快照用例：六个试点动词各一个实现者，另含跳过规则
// 工具 `browser.open` 的意图执行器（insight_intent.json 的 insightIntentsSrcEntry 引用本文件）。

import { insightIntent, InsightIntentEntry, InsightIntentEntryExecutor } from '@kit.AbilityKit';
import { HubIntentRuntime } from '../appmcp/HubInsightIntents';
import { BrowserOpenParams, HubToolHandlers } from '../appmcp/HubTools';

/**
 * 在内置浏览器中打开网址
 *
 * 工具 `browser.open`「打开链接」，风险：read
 */
@InsightIntentEntry({
  intentName: "HubBrowserOpen",
  domain: "ToolsDomain",
  intentVersion: "1.0.0",
  displayName: "打开链接",
  displayDescription: "在内置浏览器中打开网址",
  llmDescription: "在内置浏览器中打开网址",
  keywords: ["打开链接", "browser.open"],
  abilityName: "EntryAbility",
  executeMode: [insightIntent.ExecuteMode.UI_ABILITY_BACKGROUND],
  parameters: {
    "description": "在内置浏览器中打开网址",
    "properties": {
      "url": {
        "type": "string"
      }
    },
    "required": [
      "url"
    ],
    "type": "object"
  },
})
export default class HubBrowserOpenIntent extends InsightIntentEntryExecutor<string> {
  /** 格式：uri */
  public url?: string;

  onExecute(): Promise<insightIntent.IntentResult<string>> {
    const urlValue = this.url;
    if (urlValue === undefined) {
      return HubIntentRuntime.missing("url");
    }
    const params: BrowserOpenParams = {
      url: urlValue,
    };
    return HubIntentRuntime.run((handlers: HubToolHandlers) => handlers.browserOpen(params));
  }
}
