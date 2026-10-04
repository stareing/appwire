// 由 app-mcp-codegen 生成（target: harmony-insight-intents），请勿手动修改。
// App：生活助手（hub）
// 简介：标准意图快照用例：六个试点动词各一个实现者，另含跳过规则
//
// 注册为 MCP 工具：registerHubTools(appMcp, handlers)；意图执行器使用 HubIntentRuntime.handlers（见 HubInsightIntents.ets）。

import { Registrar, ToolHandle } from "@app-mcp/harmony";

/** 给联系人发送一条消息 */
export interface MessageComposeParams {
  /**
   * 收件人
   * 元素个数：≥ 1
   */
  to: string[];
  /** 正文 */
  text: string;
  /** 主题 */
  subject?: string;
}

/** 在日历中新建一个日程 */
export interface CalendarAddParams {
  title: string;
  /** 格式：date-time */
  start: string;
  /** 格式：date-time */
  end?: string;
  allDay?: boolean;
  location?: string;
}

export type PlayerPlayKind = "song" | "album" | "artist" | "playlist" | "podcast" | "video";

/** 搜索并播放音乐 */
export interface PlayerPlayParams {
  query: string;
  kind?: PlayerPlayKind;
}

/** 把文件分享给联系人 */
export interface FilesShareParams {
  /** 元素个数：≥ 1 */
  files: string[];
  mimeType?: string;
  text?: string;
}

/** 在内置浏览器中打开网址 */
export interface BrowserOpenParams {
  /** 格式：uri */
  url: string;
}

export interface MapNavigateDestination {
  name?: string;
  address?: string;
  lat?: number;
  lng?: number;
}

export type MapNavigateMode = "drive" | "walk" | "transit" | "bike";

/** 导航到目的地 */
export interface MapNavigateParams {
  destination: MapNavigateDestination;
  mode?: MapNavigateMode;
}

/** 快捷回复（与 message.compose 声明同一动词，系统意图只绑定第一个） */
export interface MessageQuickParams {
  to: string[];
  text: string;
}

/** 词表外动词 */
export interface NotesCustomParams {
  body?: string;
}

/** 未声明意图 */
export interface NotesPlainParams {}

/**
 * 生活助手 的工具实现。MCP 调用时参数已由 Host 按 inputSchema 校验；
 * 意图调用时参数由系统入口按意图 parameters 赋值。返回值序列化为 JSON。
 */
export interface HubToolHandlers {
  /**
   * 给联系人发送一条消息
   *
   * 工具 `message.compose`「发消息」，风险：write
   */
  messageCompose(params: MessageComposeParams): Object | null | Promise<Object | null>;
  /**
   * 在日历中新建一个日程
   *
   * 工具 `calendar.add`「新建日程」，风险：write
   */
  calendarAdd(params: CalendarAddParams): Object | null | Promise<Object | null>;
  /**
   * 搜索并播放音乐
   *
   * 工具 `player.play`「播放」，风险：write
   */
  playerPlay(params: PlayerPlayParams): Object | null | Promise<Object | null>;
  /**
   * 把文件分享给联系人
   *
   * 工具 `files.share`「分享文件」，风险：write
   */
  filesShare(params: FilesShareParams): Object | null | Promise<Object | null>;
  /**
   * 在内置浏览器中打开网址
   *
   * 工具 `browser.open`「打开链接」，风险：read
   */
  browserOpen(params: BrowserOpenParams): Object | null | Promise<Object | null>;
  /**
   * 导航到目的地
   *
   * 工具 `map.navigate`「开始导航」，风险：write
   */
  mapNavigate(params: MapNavigateParams): Object | null | Promise<Object | null>;
  /**
   * 快捷回复（与 message.compose 声明同一动词，系统意图只绑定第一个）
   *
   * 工具 `message.quick`，风险：write
   */
  messageQuick(params: MessageQuickParams): Object | null | Promise<Object | null>;
  /**
   * 词表外动词
   *
   * 工具 `notes.custom`，风险：write
   */
  notesCustom(params: NotesCustomParams): Object | null | Promise<Object | null>;
  /**
   * 未声明意图
   *
   * 工具 `notes.plain`，风险：read
   */
  notesPlain(params: NotesPlainParams): Object | null | Promise<Object | null>;
}

/**
 * 把清单中的全部工具注册到 @app-mcp/harmony 的客户端或 scope，调用转给 handlers。
 * @error 重名或 schema 非法时抛出原生错误（code 为 DUPLICATE_NAME / INVALID_SCHEMA）。
 */
export function registerHubTools(registrar: Registrar, handlers: HubToolHandlers): ToolHandle[] {
  return [
    registrar.tool<MessageComposeParams, Object | null>("message.compose", {
      description: "给联系人发送一条消息",
      title: "发消息",
      inputSchema: "{\"properties\":{\"subject\":{\"description\":\"主题\",\"type\":\"string\"},\"text\":{\"description\":\"正文\",\"type\":\"string\"},\"to\":{\"description\":\"收件人\",\"items\":{\"type\":\"string\"},\"minItems\":1,\"type\":\"array\"}},\"required\":[\"to\",\"text\"],\"type\":\"object\"}",
      risk: "write",
      handler: (input: MessageComposeParams): Object | null | Promise<Object | null> => handlers.messageCompose(input),
    }),
    registrar.tool<CalendarAddParams, Object | null>("calendar.add", {
      description: "在日历中新建一个日程",
      title: "新建日程",
      inputSchema: "{\"properties\":{\"allDay\":{\"type\":\"boolean\"},\"end\":{\"format\":\"date-time\",\"type\":\"string\"},\"location\":{\"type\":\"string\"},\"start\":{\"format\":\"date-time\",\"type\":\"string\"},\"title\":{\"type\":\"string\"}},\"required\":[\"title\",\"start\"],\"type\":\"object\"}",
      risk: "write",
      handler: (input: CalendarAddParams): Object | null | Promise<Object | null> => handlers.calendarAdd(input),
    }),
    registrar.tool<PlayerPlayParams, Object | null>("player.play", {
      description: "搜索并播放音乐",
      title: "播放",
      inputSchema: "{\"properties\":{\"kind\":{\"enum\":[\"song\",\"album\",\"artist\",\"playlist\",\"podcast\",\"video\"],\"type\":\"string\"},\"query\":{\"type\":\"string\"}},\"required\":[\"query\"],\"type\":\"object\"}",
      risk: "write",
      handler: (input: PlayerPlayParams): Object | null | Promise<Object | null> => handlers.playerPlay(input),
    }),
    registrar.tool<FilesShareParams, Object | null>("files.share", {
      description: "把文件分享给联系人",
      title: "分享文件",
      inputSchema: "{\"properties\":{\"files\":{\"items\":{\"type\":\"string\"},\"minItems\":1,\"type\":\"array\"},\"mimeType\":{\"type\":\"string\"},\"text\":{\"type\":\"string\"}},\"required\":[\"files\"],\"type\":\"object\"}",
      risk: "write",
      handler: (input: FilesShareParams): Object | null | Promise<Object | null> => handlers.filesShare(input),
    }),
    registrar.tool<BrowserOpenParams, Object | null>("browser.open", {
      description: "在内置浏览器中打开网址",
      title: "打开链接",
      inputSchema: "{\"properties\":{\"url\":{\"format\":\"uri\",\"type\":\"string\"}},\"required\":[\"url\"],\"type\":\"object\"}",
      risk: "read",
      handler: (input: BrowserOpenParams): Object | null | Promise<Object | null> => handlers.browserOpen(input),
    }),
    registrar.tool<MapNavigateParams, Object | null>("map.navigate", {
      description: "导航到目的地",
      title: "开始导航",
      inputSchema: "{\"properties\":{\"destination\":{\"properties\":{\"address\":{\"type\":\"string\"},\"lat\":{\"type\":\"number\"},\"lng\":{\"type\":\"number\"},\"name\":{\"type\":\"string\"}},\"type\":\"object\"},\"mode\":{\"enum\":[\"drive\",\"walk\",\"transit\",\"bike\"],\"type\":\"string\"}},\"required\":[\"destination\"],\"type\":\"object\"}",
      risk: "write",
      handler: (input: MapNavigateParams): Object | null | Promise<Object | null> => handlers.mapNavigate(input),
    }),
    registrar.tool<MessageQuickParams, Object | null>("message.quick", {
      description: "快捷回复（与 message.compose 声明同一动词，系统意图只绑定第一个）",
      inputSchema: "{\"properties\":{\"text\":{\"type\":\"string\"},\"to\":{\"items\":{\"type\":\"string\"},\"type\":\"array\"}},\"required\":[\"to\",\"text\"],\"type\":\"object\"}",
      risk: "write",
      handler: (input: MessageQuickParams): Object | null | Promise<Object | null> => handlers.messageQuick(input),
    }),
    registrar.tool<NotesCustomParams, Object | null>("notes.custom", {
      description: "词表外动词",
      inputSchema: "{\"properties\":{\"body\":{\"type\":\"string\"}},\"type\":\"object\"}",
      risk: "write",
      handler: (input: NotesCustomParams): Object | null | Promise<Object | null> => handlers.notesCustom(input),
    }),
    registrar.tool<NotesPlainParams, Object | null>("notes.plain", {
      description: "未声明意图",
      inputSchema: "{\"properties\":{},\"type\":\"object\"}",
      risk: "read",
      handler: (input: NotesPlainParams): Object | null | Promise<Object | null> => handlers.notesPlain(input),
    }),
  ];
}
