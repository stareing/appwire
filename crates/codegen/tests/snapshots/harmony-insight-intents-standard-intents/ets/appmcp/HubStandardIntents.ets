// 由 app-mcp-codegen 生成（target: harmony-insight-intents），请勿手动修改。
// App：生活助手（hub）
// 简介：标准意图快照用例：六个试点动词各一个实现者，另含跳过规则
//
// 媒体标准意图（--standard-intents）的实体解析：系统入口只传 App 侧实体 ID，由 App 解析为工具参数，
// 执行器随后调用 HubToolHandlers 的同一方法。在 AbilityStage.onCreate 中设置 `HubStandardIntents.mediaResolver`。

import { ToolCallError } from "@app-mcp/harmony";
import { PlayerPlayParams } from './HubTools';

/** PlayVideo 的请求（系统入口按标准意图 schema 赋值）。 */
export interface HubPlayVideoRequest {
  /** 意图实体 ID（App 提供给系统的内容 ID，长度不超过 64 字符）。 */
  entityId: string;
  /** 集数标识。 */
  episodeId?: string;
  /** 目标集数。 */
  episodeNumber?: number;
}

/** PlayAudio 的请求（系统入口按标准意图 schema 赋值）。 */
export interface HubPlayAudioRequest {
  /** 意图实体 ID（App 提供给系统的内容 ID，长度不超过 64 字符）。 */
  entityId: string;
  /** 有声节目 ID。 */
  soundId?: string;
}

/** PlayMusicList 的请求（系统入口按标准意图 schema 赋值）。 */
export interface HubPlayMusicListRequest {
  /** 意图实体 ID（App 提供给系统的内容 ID，长度不超过 64 字符）。 */
  entityId?: string;
  /** 歌单的 UI 形式（App 自定义）。 */
  entityGroupId?: string;
  /** 场景类型，如 MORNING_SCENE、DRIVE_SCENE。 */
  sceneType?: string;
  /** 城市名。 */
  city?: string;
}

/**
 * 媒体标准意图的实体解析：把系统传入的实体 ID 转换为工具参数（实体 ID 是 App 提供给系统的内容 ID）。
 * @error 无法解析时抛出 ToolCallError（如 RESOURCE_NOT_FOUND），意图返回失败结果。
 */
export interface HubMediaEntityResolver {
  /** PlayVideo → 工具 `player.play`。 */
  playVideo(request: HubPlayVideoRequest): PlayerPlayParams | Promise<PlayerPlayParams>;
  /** PlayAudio → 工具 `player.play`。 */
  playAudio(request: HubPlayAudioRequest): PlayerPlayParams | Promise<PlayerPlayParams>;
  /** PlayMusicList → 工具 `player.play`。 */
  playMusicList(request: HubPlayMusicListRequest): PlayerPlayParams | Promise<PlayerPlayParams>;
}

export class HubStandardIntents {
  /** 由 App 在启动时设置；未设置时媒体标准意图返回失败结果（HANDLER_ERROR）。 */
  static mediaResolver: HubMediaEntityResolver | undefined = undefined;

  /** 取解析器；未设置时为失败的 Promise。 */
  static media(): Promise<HubMediaEntityResolver> {
    const resolver = HubStandardIntents.mediaResolver;
    if (resolver === undefined) {
      return Promise.reject(new ToolCallError('HANDLER_ERROR', '未设置 HubStandardIntents.mediaResolver'));
    }
    return Promise.resolve(resolver);
  }
}
