// 由 app-mcp-codegen 生成（target: harmony-insight-intents），请勿手动修改。
// App：生活助手（hub）
// 简介：标准意图快照用例：六个试点动词各一个实现者，另含跳过规则
// 工具 `map.navigate` 的意图执行器（insight_intent.json 的 insightIntentsSrcEntry 引用本文件）。

import { insightIntent, InsightIntentEntity, InsightIntentEntry, InsightIntentEntryExecutor } from '@kit.AbilityKit';
import { HubIntentRuntime } from '../appmcp/HubInsightIntents';
import { HubToolHandlers, MapNavigateDestination, MapNavigateMode, MapNavigateParams } from '../appmcp/HubTools';

/** 参数 `destination` 的意图实体（系统入口按 parameters 中该属性的 schema 赋值）。 */
@InsightIntentEntity({
  entityCategory: "HubMapNavigate.destination",
})
export class HubMapNavigateDestinationEntity implements insightIntent.IntentEntity {
  public entityId: string = '';
  public name?: string;
  public address?: string;
  public lat?: number;
  public lng?: number;
}

/** HubMapNavigateDestinationEntity → MapNavigateDestination。 */
function toMapNavigateDestination(entity: HubMapNavigateDestinationEntity): MapNavigateDestination {
  const converted: MapNavigateDestination = {
    name: entity.name,
    address: entity.address,
    lat: entity.lat,
    lng: entity.lng,
  };
  return converted;
}

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
  public destination?: HubMapNavigateDestinationEntity;
  public mode?: string;

  onExecute(): Promise<insightIntent.IntentResult<string>> {
    const destinationEntity = this.destination;
    if (destinationEntity === undefined) {
      return HubIntentRuntime.missing("destination");
    }
    const destinationValue = toMapNavigateDestination(destinationEntity);
    const modeValue = this.mode;
    if (typeof modeValue === 'string' && !["drive", "walk", "transit", "bike"].includes(modeValue)) {
      return HubIntentRuntime.invalid("mode", "取值须为 drive / walk / transit / bike");
    }
    const params: MapNavigateParams = {
      destination: destinationValue,
      mode: modeValue === undefined ? modeValue : modeValue as MapNavigateMode,
    };
    return HubIntentRuntime.run((handlers: HubToolHandlers) => handlers.mapNavigate(params));
  }
}
