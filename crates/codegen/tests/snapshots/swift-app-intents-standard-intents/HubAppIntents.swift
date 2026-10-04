// 由 app-mcp-codegen 生成（target: swift-app-intents），请勿手动修改。
// App：生活助手（hub）
// 简介：标准意图快照用例：六个试点动词各一个实现者，另含跳过规则
//
// 依赖同目录下的 HubTools.swift（参数类型与 HubToolHandlers 协议）。
// 需要 iOS / macOS 26 起的 App Intents（supportedModes）；
// 启动时设置 `HubIntentRuntime.handlers = <实现 HubToolHandlers 的对象>`（可直接复用 MCP 的业务实现）。

import AppIntents
import Foundation

extension PlayerPlayKind: AppEnum {
    public static let typeDisplayRepresentation = TypeDisplayRepresentation(name: "PlayerPlayKind")
    public static let caseDisplayRepresentations: [PlayerPlayKind: DisplayRepresentation] = [
        .song: DisplayRepresentation(title: "song"),
        .album: DisplayRepresentation(title: "album"),
        .artist: DisplayRepresentation(title: "artist"),
        .playlist: DisplayRepresentation(title: "playlist"),
        .podcast: DisplayRepresentation(title: "podcast"),
        .video: DisplayRepresentation(title: "video"),
    ]
}

extension MapNavigateMode: AppEnum {
    public static let typeDisplayRepresentation = TypeDisplayRepresentation(name: "MapNavigateMode")
    public static let caseDisplayRepresentations: [MapNavigateMode: DisplayRepresentation] = [
        .drive: DisplayRepresentation(title: "drive"),
        .walk: DisplayRepresentation(title: "walk"),
        .transit: DisplayRepresentation(title: "transit"),
        .bike: DisplayRepresentation(title: "bike"),
    ]
}

/// App Intents 调用 HubToolHandlers 的入口。
public enum HubIntentRuntime {
    /// 由 App 在启动时设置（如 `App.init`）；未设置时 intent 抛出 `HubToolError.handlersNotSet`。
    @MainActor public static var handlers: (any HubToolHandlers)?

    @MainActor static func requireHandlers() throws -> any HubToolHandlers {
        guard let handlers else { throw HubToolError.handlersNotSet }
        return handlers
    }

    static func decode<T: Decodable>(_ type: T.Type, from json: String) throws -> T {
        try JSONDecoder().decode(type, from: Data(json.utf8))
    }

    static func decodeIfPresent<T: Decodable>(_ type: T.Type, from json: String?) throws -> T? {
        guard let json, !json.isEmpty else { return nil }
        return try decode(type, from: json)
    }

    /// 把 handler 的返回值转为 Siri / 快捷指令展示的文本：String 原样返回，其他值编码为 JSON。
    static func text(_ value: any Encodable & Sendable) throws -> String {
        if let string = value as? String { return string }
        let data = try JSONEncoder().encode(value)
        return String(decoding: data, as: UTF8.self)
    }
}

/// 给联系人发送一条消息
///
/// 工具 `message.compose`「发消息」，风险：write
public struct MessageComposeIntent: AppIntent {
    public static let title: LocalizedStringResource = "发消息"
    public static let description = IntentDescription("给联系人发送一条消息")
    public static let supportedModes: IntentModes = .foreground(.immediate)

    @Parameter(title: "to", description: "收件人。元素个数：≥ 1")
    public var to: [String]

    @Parameter(title: "text", description: "正文")
    public var text: String

    @Parameter(title: "subject", description: "主题")
    public var subject: String?

    public init() {}

    @MainActor
    public func perform() async throws -> some IntentResult & ReturnsValue<String> {
        let params = MessageComposeParams(to: to, text: text, subject: subject)
        let result = try await HubIntentRuntime.requireHandlers().messageCompose(params)
        return .result(value: try HubIntentRuntime.text(result))
    }
}

/// 在日历中新建一个日程
///
/// 工具 `calendar.add`「新建日程」，风险：write
public struct CalendarAddIntent: AppIntent {
    public static let title: LocalizedStringResource = "新建日程"
    public static let description = IntentDescription("在日历中新建一个日程")
    public static let supportedModes: IntentModes = .foreground(.immediate)

    @Parameter(title: "title")
    public var titleValue: String

    @Parameter(title: "start", description: "格式：date-time")
    public var start: Date

    @Parameter(title: "end", description: "格式：date-time")
    public var end: Date?

    @Parameter(title: "allDay")
    public var allDay: Bool?

    @Parameter(title: "location")
    public var location: String?

    public init() {}

    @MainActor
    public func perform() async throws -> some IntentResult & ReturnsValue<String> {
        let params = CalendarAddParams(title: titleValue, start: start.ISO8601Format(), end: end.map { $0.ISO8601Format() }, allDay: allDay, location: location)
        let result = try await HubIntentRuntime.requireHandlers().calendarAdd(params)
        return .result(value: try HubIntentRuntime.text(result))
    }
}

/// 搜索并播放音乐
///
/// 工具 `player.play`「播放」，风险：write
public struct PlayerPlayIntent: AppIntent {
    public static let title: LocalizedStringResource = "播放"
    public static let description = IntentDescription("搜索并播放音乐")
    public static let supportedModes: IntentModes = .foreground(.immediate)

    @Parameter(title: "query")
    public var query: String

    @Parameter(title: "kind")
    public var kind: PlayerPlayKind?

    public init() {}

    @MainActor
    public func perform() async throws -> some IntentResult & ReturnsValue<String> {
        let params = PlayerPlayParams(query: query, kind: kind)
        let result = try await HubIntentRuntime.requireHandlers().playerPlay(params)
        return .result(value: try HubIntentRuntime.text(result))
    }
}

/// 把文件分享给联系人
///
/// 工具 `files.share`「分享文件」，风险：write
public struct FilesShareIntent: AppIntent {
    public static let title: LocalizedStringResource = "分享文件"
    public static let description = IntentDescription("把文件分享给联系人")
    public static let supportedModes: IntentModes = .foreground(.immediate)

    @Parameter(title: "files", description: "元素个数：≥ 1")
    public var files: [String]

    @Parameter(title: "mimeType")
    public var mimeType: String?

    @Parameter(title: "text")
    public var text: String?

    public init() {}

    @MainActor
    public func perform() async throws -> some IntentResult & ReturnsValue<String> {
        let params = FilesShareParams(files: files, mimeType: mimeType, text: text)
        let result = try await HubIntentRuntime.requireHandlers().filesShare(params)
        return .result(value: try HubIntentRuntime.text(result))
    }
}

/// 在内置浏览器中打开网址
///
/// 工具 `browser.open`「打开链接」，风险：read
public struct BrowserOpenIntent: AppIntent {
    public static let title: LocalizedStringResource = "打开链接"
    public static let description = IntentDescription("在内置浏览器中打开网址")
    public static let supportedModes: IntentModes = .foreground(.immediate)

    @Parameter(title: "url", description: "格式：uri")
    public var url: String

    public init() {}

    @MainActor
    public func perform() async throws -> some IntentResult & ReturnsValue<String> {
        let params = BrowserOpenParams(url: url)
        let result = try await HubIntentRuntime.requireHandlers().browserOpen(params)
        return .result(value: try HubIntentRuntime.text(result))
    }
}

/// 导航到目的地
///
/// 工具 `map.navigate`「开始导航」，风险：write
public struct MapNavigateIntent: AppIntent {
    public static let title: LocalizedStringResource = "开始导航"
    public static let description = IntentDescription("导航到目的地")
    public static let supportedModes: IntentModes = .foreground(.immediate)

    // MapNavigateDestination 不能直接作为 App Intent 参数，降级为 JSON 字符串
    @Parameter(title: "destination", description: "以 JSON 字符串传入")
    public var destination: String

    @Parameter(title: "mode")
    public var mode: MapNavigateMode?

    public init() {}

    @MainActor
    public func perform() async throws -> some IntentResult & ReturnsValue<String> {
        let params = MapNavigateParams(destination: try HubIntentRuntime.decode(MapNavigateDestination.self, from: destination), mode: mode)
        let result = try await HubIntentRuntime.requireHandlers().mapNavigate(params)
        return .result(value: try HubIntentRuntime.text(result))
    }
}

/// 快捷回复（与 message.compose 声明同一动词，系统意图只绑定第一个）
///
/// 工具 `message.quick`，风险：write
public struct MessageQuickIntent: AppIntent {
    public static let title: LocalizedStringResource = "message.quick"
    public static let description = IntentDescription("快捷回复（与 message.compose 声明同一动词，系统意图只绑定第一个）")
    public static let supportedModes: IntentModes = .foreground(.immediate)

    @Parameter(title: "to")
    public var to: [String]

    @Parameter(title: "text")
    public var text: String

    public init() {}

    @MainActor
    public func perform() async throws -> some IntentResult & ReturnsValue<String> {
        let params = MessageQuickParams(to: to, text: text)
        let result = try await HubIntentRuntime.requireHandlers().messageQuick(params)
        return .result(value: try HubIntentRuntime.text(result))
    }
}

/// 词表外动词
///
/// 工具 `notes.custom`，风险：write
public struct NotesCustomIntent: AppIntent {
    public static let title: LocalizedStringResource = "notes.custom"
    public static let description = IntentDescription("词表外动词")
    public static let supportedModes: IntentModes = .foreground(.immediate)

    @Parameter(title: "body")
    public var body: String?

    public init() {}

    @MainActor
    public func perform() async throws -> some IntentResult & ReturnsValue<String> {
        let params = NotesCustomParams(body: body)
        let result = try await HubIntentRuntime.requireHandlers().notesCustom(params)
        return .result(value: try HubIntentRuntime.text(result))
    }
}

/// 未声明意图
///
/// 工具 `notes.plain`，风险：read
public struct NotesPlainIntent: AppIntent {
    public static let title: LocalizedStringResource = "notes.plain"
    public static let description = IntentDescription("未声明意图")
    public static let supportedModes: IntentModes = .foreground(.immediate)

    public init() {}

    @MainActor
    public func perform() async throws -> some IntentResult & ReturnsValue<String> {
        let params = NotesPlainParams()
        let result = try await HubIntentRuntime.requireHandlers().notesPlain(params)
        return .result(value: try HubIntentRuntime.text(result))
    }
}

/// 只读 / 写入且没有必填参数的工具提供 App Shortcut（短语由工具描述生成，建议在 String Catalog 中本地化与精简）。
public struct HubAppShortcuts: AppShortcutsProvider {
    public static var appShortcuts: [AppShortcut] {
        AppShortcut(
            intent: NotesCustomIntent(),
            phrases: ["\(.applicationName) 词表外动词"],
            shortTitle: "notes.custom",
            systemImageName: "square.and.pencil"
        )
        AppShortcut(
            intent: NotesPlainIntent(),
            phrases: ["\(.applicationName) 未声明意图"],
            shortTitle: "notes.plain",
            systemImageName: "magnifyingglass"
        )
    }
}
