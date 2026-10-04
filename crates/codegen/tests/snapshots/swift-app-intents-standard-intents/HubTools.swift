// 由 app-mcp-codegen 生成（target: swift），请勿手动修改。
// App：生活助手（hub）
// 简介：标准意图快照用例：六个试点动词各一个实现者，另含跳过规则

import Foundation

/// 给联系人发送一条消息
public struct MessageComposeParams: Codable, Sendable, Equatable {
    /// 收件人
    /// 元素个数：≥ 1
    public var to: [String]
    /// 正文
    public var text: String
    /// 主题
    public var subject: String?

    public init(to: [String], text: String, subject: String? = nil) {
        self.to = to
        self.text = text
        self.subject = subject
    }

    enum CodingKeys: String, CodingKey {
        case to = "to"
        case text = "text"
        case subject = "subject"
    }
}

/// 在日历中新建一个日程
public struct CalendarAddParams: Codable, Sendable, Equatable {
    public var title: String
    /// 格式：date-time
    public var start: String
    /// 格式：date-time
    public var end: String?
    public var allDay: Bool?
    public var location: String?

    public init(title: String, start: String, end: String? = nil, allDay: Bool? = nil, location: String? = nil) {
        self.title = title
        self.start = start
        self.end = end
        self.allDay = allDay
        self.location = location
    }

    enum CodingKeys: String, CodingKey {
        case title = "title"
        case start = "start"
        case end = "end"
        case allDay = "allDay"
        case location = "location"
    }
}

public enum PlayerPlayKind: String, Codable, Sendable, CaseIterable {
    case song = "song"
    case album = "album"
    case artist = "artist"
    case playlist = "playlist"
    case podcast = "podcast"
    case video = "video"
}

/// 搜索并播放音乐
public struct PlayerPlayParams: Codable, Sendable, Equatable {
    public var query: String
    public var kind: PlayerPlayKind?

    public init(query: String, kind: PlayerPlayKind? = nil) {
        self.query = query
        self.kind = kind
    }

    enum CodingKeys: String, CodingKey {
        case query = "query"
        case kind = "kind"
    }
}

/// 把文件分享给联系人
public struct FilesShareParams: Codable, Sendable, Equatable {
    /// 元素个数：≥ 1
    public var files: [String]
    public var mimeType: String?
    public var text: String?

    public init(files: [String], mimeType: String? = nil, text: String? = nil) {
        self.files = files
        self.mimeType = mimeType
        self.text = text
    }

    enum CodingKeys: String, CodingKey {
        case files = "files"
        case mimeType = "mimeType"
        case text = "text"
    }
}

/// 在内置浏览器中打开网址
public struct BrowserOpenParams: Codable, Sendable, Equatable {
    /// 格式：uri
    public var url: String

    public init(url: String) {
        self.url = url
    }

    enum CodingKeys: String, CodingKey {
        case url = "url"
    }
}

public struct MapNavigateDestination: Codable, Sendable, Equatable {
    public var name: String?
    public var address: String?
    public var lat: Double?
    public var lng: Double?

    public init(name: String? = nil, address: String? = nil, lat: Double? = nil, lng: Double? = nil) {
        self.name = name
        self.address = address
        self.lat = lat
        self.lng = lng
    }

    enum CodingKeys: String, CodingKey {
        case name = "name"
        case address = "address"
        case lat = "lat"
        case lng = "lng"
    }
}

public enum MapNavigateMode: String, Codable, Sendable, CaseIterable {
    case drive = "drive"
    case walk = "walk"
    case transit = "transit"
    case bike = "bike"
}

/// 导航到目的地
public struct MapNavigateParams: Codable, Sendable, Equatable {
    public var destination: MapNavigateDestination
    public var mode: MapNavigateMode?

    public init(destination: MapNavigateDestination, mode: MapNavigateMode? = nil) {
        self.destination = destination
        self.mode = mode
    }

    enum CodingKeys: String, CodingKey {
        case destination = "destination"
        case mode = "mode"
    }
}

/// 快捷回复（与 message.compose 声明同一动词，系统意图只绑定第一个）
public struct MessageQuickParams: Codable, Sendable, Equatable {
    public var to: [String]
    public var text: String

    public init(to: [String], text: String) {
        self.to = to
        self.text = text
    }

    enum CodingKeys: String, CodingKey {
        case to = "to"
        case text = "text"
    }
}

/// 词表外动词
public struct NotesCustomParams: Codable, Sendable, Equatable {
    public var body: String?

    public init(body: String? = nil) {
        self.body = body
    }

    enum CodingKeys: String, CodingKey {
        case body = "body"
    }
}

/// 未声明意图
public struct NotesPlainParams: Codable, Sendable, Equatable {
    public init() {}
}

/// 生活助手 的工具实现。每个方法接收已由 Host 按 inputSchema 校验过的参数，
/// 返回值会编码为 JSON 作为工具结果（返回 String 时原样作为文本）。
public protocol HubToolHandlers: Sendable {
    /// 给联系人发送一条消息
    ///
    /// 工具 `message.compose`「发消息」，风险：write
    func messageCompose(_ params: MessageComposeParams) async throws -> any Encodable & Sendable
    /// 在日历中新建一个日程
    ///
    /// 工具 `calendar.add`「新建日程」，风险：write
    func calendarAdd(_ params: CalendarAddParams) async throws -> any Encodable & Sendable
    /// 搜索并播放音乐
    ///
    /// 工具 `player.play`「播放」，风险：write
    func playerPlay(_ params: PlayerPlayParams) async throws -> any Encodable & Sendable
    /// 把文件分享给联系人
    ///
    /// 工具 `files.share`「分享文件」，风险：write
    func filesShare(_ params: FilesShareParams) async throws -> any Encodable & Sendable
    /// 在内置浏览器中打开网址
    ///
    /// 工具 `browser.open`「打开链接」，风险：read
    func browserOpen(_ params: BrowserOpenParams) async throws -> any Encodable & Sendable
    /// 导航到目的地
    ///
    /// 工具 `map.navigate`「开始导航」，风险：write
    func mapNavigate(_ params: MapNavigateParams) async throws -> any Encodable & Sendable
    /// 快捷回复（与 message.compose 声明同一动词，系统意图只绑定第一个）
    ///
    /// 工具 `message.quick`，风险：write
    func messageQuick(_ params: MessageQuickParams) async throws -> any Encodable & Sendable
    /// 词表外动词
    ///
    /// 工具 `notes.custom`，风险：write
    func notesCustom(_ params: NotesCustomParams) async throws -> any Encodable & Sendable
    /// 未声明意图
    ///
    /// 工具 `notes.plain`，风险：read
    func notesPlain(_ params: NotesPlainParams) async throws -> any Encodable & Sendable
}

public enum HubToolError: Error, Equatable {
    /// 未知的工具名。
    case unknownTool(String)
    /// 尚未设置 handler（原生意图框架调用时）。
    case handlersNotSet
}

/// 工具名与分派辅助。
public enum HubTools {
    /// 清单中的全部工具名。
    public static let names: [String] = [
        "message.compose",
        "calendar.add",
        "player.play",
        "files.share",
        "browser.open",
        "map.navigate",
        "message.quick",
        "notes.custom",
        "notes.plain",
    ]

    /// 按工具名把调用分派到对应的 handler。`arguments` 为参数 JSON（空数据视为 `{}`）。
    public static func dispatch(_ handlers: any HubToolHandlers, name: String, arguments: Data) async throws -> any Encodable & Sendable {
        let data = arguments.isEmpty ? Data("{}".utf8) : arguments
        let decoder = JSONDecoder()
        switch name {
        case "message.compose":
            return try await handlers.messageCompose(decoder.decode(MessageComposeParams.self, from: data))
        case "calendar.add":
            return try await handlers.calendarAdd(decoder.decode(CalendarAddParams.self, from: data))
        case "player.play":
            return try await handlers.playerPlay(decoder.decode(PlayerPlayParams.self, from: data))
        case "files.share":
            return try await handlers.filesShare(decoder.decode(FilesShareParams.self, from: data))
        case "browser.open":
            return try await handlers.browserOpen(decoder.decode(BrowserOpenParams.self, from: data))
        case "map.navigate":
            return try await handlers.mapNavigate(decoder.decode(MapNavigateParams.self, from: data))
        case "message.quick":
            return try await handlers.messageQuick(decoder.decode(MessageQuickParams.self, from: data))
        case "notes.custom":
            return try await handlers.notesCustom(decoder.decode(NotesCustomParams.self, from: data))
        case "notes.plain":
            return try await handlers.notesPlain(decoder.decode(NotesPlainParams.self, from: data))
        default:
            throw HubToolError.unknownTool(name)
        }
    }
}
