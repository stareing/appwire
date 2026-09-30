// AppIntents 框架的最小桩模块（仅供 verify.sh 在 Linux 上对生成代码做类型检查）。
//
// 签名按 Apple 官方文档（2026-09）编写，只覆盖生成代码用到的 API：
//   AppIntent（title / description / supportedModes / perform）、@Parameter(title:description:default:)、
//   AppEnum（typeDisplayRepresentation / caseDisplayRepresentations）、IntentResult & ReturnsValue、
//   .result(value:)、requestConfirmation(conditions:actionName:dialog:)、IntentModes、
//   AppShortcutsProvider / AppShortcut(intent:phrases:shortTitle:systemImageName:)。
// 通过这里的检查不代表能在真实 SDK 上编译，真实验证需要 Xcode。

public struct LocalizedStringResource: ExpressibleByStringInterpolation, Sendable, Hashable {
    public init(stringLiteral value: String) {}
}

public struct IntentDescription: Sendable {
    public init(_ description: LocalizedStringResource, categoryName: LocalizedStringResource? = nil) {}
}

public struct IntentDialog: ExpressibleByStringInterpolation, Sendable {
    public init(stringLiteral value: String) {}
}

public struct IntentModes: OptionSet, Sendable {
    public let rawValue: Int
    public init(rawValue: Int) { self.rawValue = rawValue }

    public enum ForegroundMode: Sendable {
        case immediate, dynamic, deferred
    }

    public static let background = IntentModes(rawValue: 1)
    public static func foreground(_ mode: ForegroundMode) -> IntentModes { IntentModes(rawValue: 2) }
}

public struct ConfirmationConditions: OptionSet, Sendable {
    public let rawValue: Int
    public init(rawValue: Int) { self.rawValue = rawValue }
    public static let lowConfidenceSource = ConfirmationConditions(rawValue: 1)
}

public struct ConfirmationActionName: Sendable {
    public static let `continue` = ConfirmationActionName()
    public static let pay = ConfirmationActionName()
    public static let buy = ConfirmationActionName()
}

public protocol IntentResult: Sendable {}

public protocol ReturnsValue<Value>: IntentResult {
    associatedtype Value
}

public struct IntentResultContainer<Value>: ReturnsValue, @unchecked Sendable {}

extension IntentResult {
    public static func result<V>(value: V) -> Self where Self == IntentResultContainer<V> {
        IntentResultContainer<V>()
    }
}

public protocol AppIntent: Sendable {
    associatedtype PerformResult: IntentResult
    init()
    static var title: LocalizedStringResource { get }
    static var description: IntentDescription? { get }
    static var supportedModes: IntentModes { get }
    func perform() async throws -> PerformResult
}

extension AppIntent {
    public static var description: IntentDescription? { nil }
    public static var supportedModes: IntentModes { .background }

    public func requestConfirmation(
        conditions: ConfirmationConditions = [],
        actionName: ConfirmationActionName = .`continue`,
        dialog: IntentDialog
    ) async throws {}
}

@propertyWrapper
public final class Parameter<Value>: @unchecked Sendable {
    private var value: Value?

    public init(title: LocalizedStringResource, description: LocalizedStringResource? = nil) {}

    public init(title: LocalizedStringResource, description: LocalizedStringResource? = nil, default: Value) {
        value = `default`
    }

    public var wrappedValue: Value {
        get {
            guard let value else { fatalError("桩模块不提供参数值") }
            return value
        }
        set { value = newValue }
    }
}

public struct TypeDisplayRepresentation: Sendable {
    public init(name: LocalizedStringResource) {}
}

public struct DisplayRepresentation: Sendable {
    public init(title: LocalizedStringResource) {}
}

public protocol AppEnum: RawRepresentable, CaseIterable, Hashable, Sendable
where RawValue: LosslessStringConvertible {
    static var typeDisplayRepresentation: TypeDisplayRepresentation { get }
    static var caseDisplayRepresentations: [Self: DisplayRepresentation] { get }
}

public struct AppShortcutPhraseToken: Sendable {
    public static let applicationName = AppShortcutPhraseToken()
}

public struct AppShortcutPhrase<Intent: AppIntent>: ExpressibleByStringInterpolation, Sendable {
    public struct StringInterpolation: StringInterpolationProtocol {
        public init(literalCapacity: Int, interpolationCount: Int) {}
        public mutating func appendLiteral(_ literal: String) {}
        public mutating func appendInterpolation(_ token: AppShortcutPhraseToken) {}
    }

    public init(stringLiteral value: String) {}
    public init(stringInterpolation: StringInterpolation) {}
}

public struct AppShortcut: Sendable {
    public init<Intent: AppIntent>(
        intent: Intent,
        phrases: [AppShortcutPhrase<Intent>],
        shortTitle: LocalizedStringResource,
        systemImageName: String
    ) {}
}

@resultBuilder
public struct AppShortcutsBuilder {
    public static func buildBlock(_ components: AppShortcut...) -> [AppShortcut] { components }
}

public protocol AppShortcutsProvider: Sendable {
    @AppShortcutsBuilder static var appShortcuts: [AppShortcut] { get }
}
