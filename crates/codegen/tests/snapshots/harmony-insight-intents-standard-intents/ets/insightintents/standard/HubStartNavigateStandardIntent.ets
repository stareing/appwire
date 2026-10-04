// 由 app-mcp-codegen 生成（target: harmony-insight-intents），请勿手动修改。
// App：生活助手（hub）
// 简介：标准意图快照用例：六个试点动词各一个实现者，另含跳过规则
// 工具 `map.navigate` 的鸿蒙标准意图 StartNavigate（--standard-intents；insight_intent.json 的 insightIntentsSrcEntry 引用本文件）。

import { insightIntent, InsightIntentEntity, InsightIntentEntry, InsightIntentEntryExecutor } from '@kit.AbilityKit';
import { HubIntentRuntime } from '../../appmcp/HubInsightIntents';
import { HubToolHandlers, MapNavigateDestination, MapNavigateMode, MapNavigateParams } from '../../appmcp/HubTools';

/** StartNavigate 的地点（系统入口按标准意图 schema 赋值；字段均为字符串）。 */
@InsightIntentEntity({
  entityCategory: "location",
})
export class HubStandardNavigateLocation implements insightIntent.IntentEntity {
  public entityId: string = '';
  public poiId?: string;
  public locationName?: string;
  /** 坐标系，缺省为 GCJ-02。 */
  public locationSystem?: string;
  public longitude?: string;
  public latitude?: string;
  public address?: string;
}

/** trafficType → mode。 */
const TRAFFIC_MODES: Record<string, MapNavigateMode> = {
  "Drive": "drive",
  "Walk": "walk",
  "Cycle": "bike",
  "Bus": "transit",
  "Subway": "transit",
};

/**
 * 坐标字符串转为数值；只在坐标系明确为 WGS-84 时传递，否则为 undefined。
 * @why 标准意图坐标缺省为 GCJ-02，词表 destination 的 lat / lng 按 WGS-84 解释；不做坐标转换。
 */
function wgs84Coordinate(system: string | undefined, value: string | undefined): number | undefined {
  if (value === undefined || system === undefined || system.replace('-', '').toUpperCase() !== 'WGS84') {
    return undefined;
  }
  const n = Number(value);
  return Number.isFinite(n) ? n : undefined;
}

/**
 * 工具 `map.navigate` 的鸿蒙标准意图 StartNavigate（navigation.start@1）。
 * 目的地由 dstLocation 转换（坐标只在坐标系为 WGS-84 时传递），交通方式由 trafficType 转换。
 */
@InsightIntentEntry({
  intentName: "StartNavigate",
  domain: "NavigationDomain",
  intentVersion: "1.0.1",
  displayName: "开始导航",
  displayDescription: "导航到目的地",
  schema: "StartNavigate",
  abilityName: "EntryAbility",
  executeMode: [insightIntent.ExecuteMode.UI_ABILITY_BACKGROUND],
})
export default class HubStartNavigateStandardIntent extends InsightIntentEntryExecutor<string> {
  /** 意图实体 ID（App 提供给系统的内容 ID，长度不超过 64 字符）。 */
  public entityId?: string;
  /** 目的地（坐标系缺省为 GCJ-02）。 */
  public dstLocation?: HubStandardNavigateLocation;
  /** 交通方式：Drive / Walk / Cycle / Bus / Subway。 */
  public trafficType?: string;

  onExecute(): Promise<insightIntent.IntentResult<string>> {
    const dst = this.dstLocation;
    if (dst === undefined) {
      return HubIntentRuntime.missing("dstLocation");
    }
    const nameValue = dst.locationName;
    const addressValue = dst.address;
    const latValue = wgs84Coordinate(dst.locationSystem, dst.latitude);
    const lngValue = wgs84Coordinate(dst.locationSystem, dst.longitude);
    // 词表要求 name / address / lat + lng 至少一组
    if (nameValue === undefined && addressValue === undefined && (latValue === undefined || lngValue === undefined)) {
      return HubIntentRuntime.missing("dstLocation");
    }
    const destination: MapNavigateDestination = {
      name: nameValue,
      address: addressValue,
      lat: latValue,
      lng: lngValue,
    };
    const params: MapNavigateParams = {
      destination: destination,
      mode: this.trafficType === undefined ? undefined : TRAFFIC_MODES[this.trafficType],
    };
    return HubIntentRuntime.run((handlers: HubToolHandlers) => handlers.mapNavigate(params));
  }
}
