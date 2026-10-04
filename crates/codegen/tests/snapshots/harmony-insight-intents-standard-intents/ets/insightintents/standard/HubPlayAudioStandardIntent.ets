// 由 app-mcp-codegen 生成（target: harmony-insight-intents），请勿手动修改。
// App：生活助手（hub）
// 简介：标准意图快照用例：六个试点动词各一个实现者，另含跳过规则
// 工具 `player.play` 的鸿蒙标准意图 PlayAudio（--standard-intents；insight_intent.json 的 insightIntentsSrcEntry 引用本文件）。

import { insightIntent, InsightIntentEntry, InsightIntentEntryExecutor } from '@kit.AbilityKit';
import { HubIntentRuntime } from '../../appmcp/HubInsightIntents';
import { HubMediaEntityResolver, HubPlayAudioRequest, HubStandardIntents } from '../../appmcp/HubStandardIntents';
import { HubToolHandlers, PlayerPlayParams } from '../../appmcp/HubTools';

/**
 * 工具 `player.play` 的鸿蒙标准意图 PlayAudio（media.play@1）。
 * 系统传入 App 实体 ID，经 HubStandardIntents.mediaResolver 解析为工具参数。
 */
@InsightIntentEntry({
  intentName: "PlayAudio",
  domain: "MediaDomain",
  intentVersion: "1.0.1",
  displayName: "播放",
  displayDescription: "搜索并播放音乐",
  schema: "PlayAudio",
  abilityName: "EntryAbility",
  executeMode: [insightIntent.ExecuteMode.UI_ABILITY_BACKGROUND],
})
export default class HubPlayAudioStandardIntent extends InsightIntentEntryExecutor<string> {
  /** 意图实体 ID（App 提供给系统的内容 ID，长度不超过 64 字符）。 */
  public entityId?: string;
  /** 有声节目 ID。 */
  public soundId?: string;

  onExecute(): Promise<insightIntent.IntentResult<string>> {
    const entityIdValue = this.entityId;
    if (entityIdValue === undefined) {
      return HubIntentRuntime.missing("entityId");
    }
    const request: HubPlayAudioRequest = {
      entityId: entityIdValue,
      soundId: this.soundId,
    };
    return HubIntentRuntime.run((handlers: HubToolHandlers) => HubStandardIntents.media()
      .then((resolver: HubMediaEntityResolver) => resolver.playAudio(request))
      .then((params: PlayerPlayParams) => handlers.playerPlay(params)));
  }
}
