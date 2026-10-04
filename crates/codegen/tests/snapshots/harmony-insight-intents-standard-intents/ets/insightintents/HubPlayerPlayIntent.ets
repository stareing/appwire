// 由 app-mcp-codegen 生成（target: harmony-insight-intents），请勿手动修改。
// App：生活助手（hub）
// 简介：标准意图快照用例：六个试点动词各一个实现者，另含跳过规则
// 工具 `player.play` 的意图执行器（insight_intent.json 的 insightIntentsSrcEntry 引用本文件）。

import { insightIntent, InsightIntentEntry, InsightIntentEntryExecutor } from '@kit.AbilityKit';
import { HubIntentRuntime } from '../appmcp/HubInsightIntents';
import { HubToolHandlers, PlayerPlayKind, PlayerPlayParams } from '../appmcp/HubTools';

/**
 * 搜索并播放音乐
 *
 * 工具 `player.play`「播放」，风险：write
 */
@InsightIntentEntry({
  intentName: "HubPlayerPlay",
  domain: "ToolsDomain",
  intentVersion: "1.0.0",
  displayName: "播放",
  displayDescription: "搜索并播放音乐",
  llmDescription: "搜索并播放音乐",
  keywords: ["播放", "player.play"],
  abilityName: "EntryAbility",
  executeMode: [insightIntent.ExecuteMode.UI_ABILITY_BACKGROUND],
  parameters: {
    "description": "搜索并播放音乐",
    "properties": {
      "kind": {
        "enum": [
          "song",
          "album",
          "artist",
          "playlist",
          "podcast",
          "video"
        ],
        "type": "string"
      },
      "query": {
        "type": "string"
      }
    },
    "required": [
      "query"
    ],
    "type": "object"
  },
})
export default class HubPlayerPlayIntent extends InsightIntentEntryExecutor<string> {
  public query?: string;
  public kind?: PlayerPlayKind;

  onExecute(): Promise<insightIntent.IntentResult<string>> {
    const queryValue = this.query;
    if (queryValue === undefined) {
      return HubIntentRuntime.missing("query");
    }
    const params: PlayerPlayParams = {
      query: queryValue,
      kind: this.kind,
    };
    return HubIntentRuntime.run((handlers: HubToolHandlers) => handlers.playerPlay(params));
  }
}
