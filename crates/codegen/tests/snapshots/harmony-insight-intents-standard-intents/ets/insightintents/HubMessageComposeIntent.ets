// 由 app-mcp-codegen 生成（target: harmony-insight-intents），请勿手动修改。
// App：生活助手（hub）
// 简介：标准意图快照用例：六个试点动词各一个实现者，另含跳过规则
// 工具 `message.compose` 的意图执行器（insight_intent.json 的 insightIntentsSrcEntry 引用本文件）。

import { insightIntent, InsightIntentEntry, InsightIntentEntryExecutor } from '@kit.AbilityKit';
import { HubIntentRuntime } from '../appmcp/HubInsightIntents';
import { HubToolHandlers, MessageComposeParams } from '../appmcp/HubTools';

/**
 * 给联系人发送一条消息
 *
 * 工具 `message.compose`「发消息」，风险：write
 */
@InsightIntentEntry({
  intentName: "HubMessageCompose",
  domain: "ToolsDomain",
  intentVersion: "1.0.0",
  displayName: "发消息",
  displayDescription: "给联系人发送一条消息",
  llmDescription: "给联系人发送一条消息",
  keywords: ["发消息", "message.compose"],
  abilityName: "EntryAbility",
  executeMode: [insightIntent.ExecuteMode.UI_ABILITY_BACKGROUND],
  parameters: {
    "description": "给联系人发送一条消息",
    "properties": {
      "subject": {
        "description": "主题",
        "type": "string"
      },
      "text": {
        "description": "正文",
        "type": "string"
      },
      "to": {
        "description": "收件人",
        "items": {
          "type": "string"
        },
        "minItems": 1,
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
export default class HubMessageComposeIntent extends InsightIntentEntryExecutor<string> {
  /**
   * 收件人
   * 元素个数：≥ 1
   */
  public to?: string[];
  /** 正文 */
  public text?: string;
  /** 主题 */
  public subject?: string;

  onExecute(): Promise<insightIntent.IntentResult<string>> {
    const toValue = this.to;
    if (toValue === undefined) {
      return HubIntentRuntime.missing("to");
    }
    const textValue = this.text;
    if (textValue === undefined) {
      return HubIntentRuntime.missing("text");
    }
    const params: MessageComposeParams = {
      to: toValue,
      text: textValue,
      subject: this.subject,
    };
    return HubIntentRuntime.run((handlers: HubToolHandlers) => handlers.messageCompose(params));
  }
}
