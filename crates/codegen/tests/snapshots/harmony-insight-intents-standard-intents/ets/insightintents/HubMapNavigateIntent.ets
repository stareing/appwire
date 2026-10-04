// 由 app-mcp-codegen 生成（target: harmony-insight-intents），请勿手动修改。
// App：生活助手（hub）
// 简介：标准意图快照用例：六个试点动词各一个实现者，另含跳过规则
// 工具 `map.navigate` 的意图执行器（insight_intent.json 的 insightIntentsSrcEntry 引用本文件）。

import { insightIntent, InsightIntentEntry, InsightIntentEntryExecutor } from '@kit.AbilityKit';
import { HubIntentRuntime } from '../appmcp/HubInsightIntents';
import { HubToolHandlers, MapNavigateDestination, MapNavigateMode, MapNavigateParams } from '../appmcp/HubTools';

/**
 * 导航到目的地
 *
 * 工具 `map.navigate`「开始导航」，风险：write
 */
@InsightIntentEntry({
  intentName: "HubMapNavigate",
  domain: "ToolsDomain",
  intentVersion: "1.0.0",
  displayName: "开始导航",
  displayDescription: "导航到目的地",
  llmDescription: "导航到目的地",
  keywords: ["开始导航", "map.navigate"],
  abilityName: "EntryAbility",
  executeMode: [insightIntent.ExecuteMode.UI_ABILITY_BACKGROUND],
  parameters: {
    "description": "导航到目的地",
    "properties": {
      "destination": {
        "properties": {
          "address": {
            "type": "string"
          },
          "lat": {
            "type": "number"
          },
          "lng": {
            "type": "number"
          },
          "name": {
            "type": "string"
          }
        },
        "type": "object"
      },
      "mode": {
        "enum": [
          "drive",
          "walk",
          "transit",
          "bike"
        ],
        "type": "string"
      }
    },
    "required": [
      "destination"
    ],
    "type": "object"
  },
})
export default class HubMapNavigateIntent extends InsightIntentEntryExecutor<string> {
  public destination?: MapNavigateDestination;
  public mode?: MapNavigateMode;

  onExecute(): Promise<insightIntent.IntentResult<string>> {
    const destinationValue = this.destination;
    if (destinationValue === undefined) {
      return HubIntentRuntime.missing("destination");
    }
    const params: MapNavigateParams = {
      destination: destinationValue,
      mode: this.mode,
    };
    return HubIntentRuntime.run((handlers: HubToolHandlers) => handlers.mapNavigate(params));
  }
}
