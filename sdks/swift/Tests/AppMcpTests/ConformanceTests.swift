import AppMcp
import Foundation
import XCTest

/// 一致性用例 runner（Swift）：按 `conformance/cases/*.json` 的 `app` 部分注册工具与资源，连接 fake_host
/// （`--case` 模式，核对在 fake_host 内完成）。格式与约定见 conformance/README.md；参照 Rust runner
/// crates/native/tests/it/conformance.rs。
///
/// 只跑部分用例：`APP_MCP_CONFORMANCE_CASES=handshake,errors swift test --filter ConformanceTests`。
final class ConformanceTests: XCTestCase {
    private static let sdk = "swift"
    /// 本 runner 支持的用例能力（`requires`），见 conformance/README.md 第 4 节。
    private static let features: Set<String> = [
        "toolOptions", "mutate", "lifecycle", "wake", "richResult", "userAction", "progress", "resourceOptions",
        "readFailure", "surface", "navigation", "backgroundTool", "backgroundNavigation", "idempotencyKey",
        "callScheduling", "busy", "events", "implements", "cache", "deprecated", "undo",
    ]
    private static let verdictOK: Set<String> = ["pass", "xfail", "xpass", "skip"]

    private var repoRoot: URL {
        URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent()
    }

    /// fake_host 定位与 IntegrationTests 相同（`APP_MCP_FAKE_HOST` 或 cargo 构建）。
    private func fakeHost() throws -> String {
        if let explicit = ProcessInfo.processInfo.environment["APP_MCP_FAKE_HOST"] { return explicit }
        let targetDir = ProcessInfo.processInfo.environment["CARGO_TARGET_DIR"] ?? repoRoot.appendingPathComponent("target").path
        let build = Process()
        build.executableURL = URL(fileURLWithPath: "/usr/bin/env")
        build.arguments = ["cargo", "build", "-q", "-p", "app-mcp-native", "--example", "fake_host"]
        build.currentDirectoryURL = repoRoot
        do {
            try build.run()
        } catch {
            throw XCTSkip("没有 cargo，跳过一致性用例：\(error)")
        }
        build.waitUntilExit()
        guard build.terminationStatus == 0 else { throw XCTSkip("fake_host 构建失败") }
        return targetDir + "/debug/examples/fake_host"
    }

    func testConformanceCases() async throws {
        let bin = try fakeHost()
        let only = ProcessInfo.processInfo.environment["APP_MCP_CONFORMANCE_CASES"].map {
            Set($0.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) })
        }
        let dir = repoRoot.appendingPathComponent("conformance/cases")
        let cases = try FileManager.default.contentsOfDirectory(at: dir, includingPropertiesForKeys: nil)
            .filter { $0.pathExtension == "json" }
            .filter { only == nil || only!.contains($0.deletingPathExtension().lastPathComponent) }
            .sorted { $0.lastPathComponent < $1.lastPathComponent }
        XCTAssertFalse(cases.isEmpty, "没有找到用例")
        var failed: [String] = []
        for path in cases {
            let id = path.deletingPathExtension().lastPathComponent
            let (verdict, exit) = try await runCase(bin: bin, path: path)
            let status = verdict["status"]?.stringValue ?? "error"
            print("[\(Self.sdk)] \(id.padding(toLength: 24, withPad: " ", startingAt: 0)) \(status)")
            if !Self.verdictOK.contains(status) || exit != 0 {
                failed.append("\(id): \(status)（退出码 \(exit)）\(verdict["failures"].map { "\($0)" } ?? "")")
            }
        }
        XCTAssertTrue(failed.isEmpty, "一致性用例失败：\n" + failed.joined(separator: "\n"))
    }

    /// 跑一个用例，返回 fake_host 的结论行与退出码。
    private func runCase(bin: String, path: URL) async throws -> (JSONValue, Int32) {
        let case_ = try JSONDecoder().decode(JSONValue.self, from: Data(contentsOf: path))
        let missing = (case_["requires"]?.arrayValue ?? []).compactMap(\.stringValue).filter { !Self.features.contains($0) }
        let report = repoRoot.appendingPathComponent("target/conformance").path
        var args = ["--case", path.path, "--sdk", Self.sdk, "--report-dir", report]
        if !missing.isEmpty { args += ["--skip", "runner 不支持：\(missing.joined(separator: ", "))"] }
        let host = Process()
        host.executableURL = URL(fileURLWithPath: bin)
        host.arguments = args
        let out = Pipe()
        host.standardOutput = out
        try host.run()
        var app: CaseApp?
        var verdict = JSONValue.null
        defer {
            app?.client.stop()
            if host.isRunning { host.terminate() }
        }
        // @why 阻塞读取在当前（非主）线程上进行：工具 handler 在主 actor 上执行，不能占用主线程
        while let line = readLine(out.fileHandleForReading) {
            if line.hasPrefix("LISTENING ") {
                app = try await CaseApp.start(addr: String(line.dropFirst("LISTENING ".count)), case: case_)
                continue
            }
            guard let v = try? JSONDecoder().decode(JSONValue.self, from: Data(line.utf8)) else { continue }
            switch v["type"]?.stringValue {
            case "wake": app?.client.handleWake(v["arg"]?.stringValue ?? "")
            case "verdict": verdict = v
            default: break
            }
        }
        host.waitUntilExit()
        return (verdict, host.terminationStatus)
    }

    private func readLine(_ handle: FileHandle) -> String? {
        var bytes = Data()
        while true {
            let b = handle.readData(ofLength: 1)
            if b.isEmpty { return bytes.isEmpty ? nil : String(decoding: bytes, as: UTF8.self) }
            if b == Data([0x0A]) { return String(decoding: bytes, as: UTF8.self) }
            bytes.append(b)
        }
    }
}

private struct CaseError: Error, CustomStringConvertible {
    let description: String
}

/// 一个用例的 App：客户端与已注册工具（mutate 用）。工具注册与变更都在主 actor 上进行。
@MainActor
private final class CaseApp {
    let client: AppMcpClient
    private var tools: [String: ToolHandle] = [:]

    private init(client: AppMcpClient) {
        self.client = client
    }

    static func start(addr: String, case c: JSONValue) throws -> CaseApp {
        let app = CaseApp(client: try AppMcpClient(config: config(addr: addr, c["app"]?["config"])))
        for t in c["app"]?["tools"]?.arrayValue ?? [] { try app.register(t) }
        for r in c["app"]?["resources"]?.arrayValue ?? [] { try app.registerResource(r) }
        for e in c["app"]?["events"]?.arrayValue ?? [] { try app.declareEvent(e) }
        if let pages = c["app"]?["navigation"] {
            app.client.setNavigationHandler { [weak app] req in
                guard let app else { return .failed("App 已释放") }
                return try app.navigate(pages, req)
            }
        }
        if let v = c["app"]?["visibility"]?.stringValue {
            app.client.setVisibility(v == "frozen" ? .frozen : v == "hidden" ? .hidden : .visible, focused: false)
        }
        if c["app"]?["busy"]?.boolValue == true { app.client.setBusy(true) }
        app.client.start()
        return app
    }

    private static func config(addr: String, _ c: JSONValue?) -> AppMcpConfig {
        let url = addr.contains(":") && !addr.hasPrefix("unix:") && !addr.hasPrefix("pipe:") ? "ws://\(addr)/app" : addr
        var lifecycle: LifecyclePolicy?
        if let l = c?["lifecycle"] {
            var p = LifecyclePolicy()
            if let m = l["mode"]?.stringValue { p.mode = m == "idle" ? .idle : m == "on-demand" ? .onDemand : .persistent }
            if let n = l["idleTimeoutMs"]?.doubleValue { p.idleTimeoutMs = UInt64(n) }
            if let n = l["graceMs"]?.doubleValue { p.graceMs = UInt64(n) }
            if let n = l["mergeWindowMs"]?.doubleValue { p.mergeWindowMs = UInt64(n) }
            lifecycle = p
        }
        var dedup: CallDedupPolicy?
        if let d = c?["callDedup"] {
            var p = CallDedupPolicy()
            if let n = d["ttlMs"]?.doubleValue { p.ttlMs = UInt64(n) }
            if let n = d["maxEntries"]?.doubleValue { p.maxEntries = UInt32(n) }
            dedup = p
        }
        return AppMcpConfig(
            appId: "conf", appName: "Conformance", hostURL: url,
            maxConcurrentCalls: Int(c?["maxConcurrentCalls"]?.doubleValue ?? 1),
            lifecycle: lifecycle ?? .persistent, callDedup: dedup,
            navigateInBackground: c?["navigateInBackground"]?.boolValue,
            maxQueuedCalls: c?["maxQueuedCalls"]?.doubleValue.map { Int($0) },
            busyPolicy: c?["busyPolicy"]?.stringValue.map { $0 == "queue" ? BusyPolicy.queue : .reject }
        )
    }

    func register(_ decl: JSONValue) throws {
        let name = decl["name"]?.stringValue ?? ""
        let spec = decl["handler"] ?? .object([:])
        var runs = 0
        tools[name] = try client.tool(
            name,
            description: decl["description"]?.stringValue ?? "",
            inputSchema: try decl["inputSchema"].map(text),
            risk: decl["risk"]?.stringValue.map(risk) ?? .write,
            activation: decl["activation"]?.stringValue.map(activation),
            title: decl["title"]?.stringValue,
            enabled: decl["enabled"]?.boolValue ?? true,
            annotations: decl["annotations"].map(toolAnnotations),
            outputSchema: try decl["outputSchema"].map(text),
            surface: decl["surface"]?.stringValue == "view" ? .view : .app,
            page: decl["page"]?.stringValue,
            backgroundTool: decl["backgroundTool"]?.stringValue,
            concurrency: Int(decl["concurrency"]?.doubleValue ?? 0),
            exclusive: decl["exclusive"]?.stringValue,
            implements: Self.strings(decl["implements"]),
            cache: cachePolicy(decl["cache"]),
            deprecated: deprecation(decl["deprecated"]),
            undoable: decl["undoable"]?.boolValue ?? false
        ) { [weak self] (args: JSONValue, ctx: ToolContext) async throws -> ToolResult<JSONValue> in
            runs += 1
            return try await self?.run(spec, count: runs, args: args, ctx: ctx) ?? ToolResult(data: nil)
        }
    }

    /// 顺序：progress → delayMs → mutate → emit → 结果（conformance/README.md 2.1）。普通返回值即 `.done` 且无附加信息的结果。
    private func run(_ spec: JSONValue, count: Int, args: JSONValue, ctx: ToolContext) async throws -> ToolResult<JSONValue> {
        for p in spec["progress"]?.arrayValue ?? [] {
            ctx.progress(p["progress"]?.doubleValue ?? 0, total: p["total"]?.doubleValue, message: p["message"]?.stringValue)
        }
        if let ms = spec["delayMs"]?.doubleValue {
            try await Task.sleep(nanoseconds: UInt64(ms) * 1_000_000) // 取消 / 超时时 Task 被取消
        }
        for op in spec["mutate"]?.arrayValue ?? [] { try mutate(op) }
        let emitted = spec["emit"]?.arrayValue.map(emitEvents)
        if let msg = spec["throw"]?.stringValue { throw CaseError(description: msg) }
        if let u = spec["userAction"], u != .null {
            throw ToolCallError.userActionRequired(message: u["message"]?.stringValue ?? "", reason: u["reason"]?.stringValue, uri: u["uri"]?.stringValue)
        }
        if let r = spec["result"], case .object = r {
            return ToolResult(
                data: r["data"],
                stateHints: (r["stateHints"]?.arrayValue ?? []).compactMap(\.stringValue),
                status: r["status"]?.stringValue.map(resultStatus) ?? .done,
                stateResource: r["stateResource"]?.stringValue,
                summary: r["summary"]?.stringValue,
                annotations: r["annotations"].map(contentAnnotations),
                undo: try undoAction(r["undo"])
            )
        }
        if let v = spec["return"] { return ToolResult(data: v) } // 含 null：显式返回 null
        if spec["echo"]?.boolValue == true { return ToolResult(data: args) }
        if spec["returnIdempotencyKey"]?.boolValue == true {
            return ToolResult(data: .object(["idempotencyKey": ctx.idempotencyKey.map(JSONValue.string) ?? .null]))
        }
        if spec["counter"]?.boolValue == true { return ToolResult(data: .object(["count": .number(Double(count))])) }
        if let emitted { return ToolResult(data: .object(["emitted": .array(emitted)])) }
        return ToolResult(data: nil) // returnNothing：Swift 的"无返回值"
    }

    /// 事件声明（conformance/README.md 2.2）。
    func declareEvent(_ decl: JSONValue) throws {
        try client.declareEvent(
            decl["name"]?.stringValue ?? "", description: decl["description"]?.stringValue ?? "",
            payloadSchema: try decl["payloadSchema"].map(text)
        )
    }

    /// handler 的 `emit`：每项为 `true` / `false`（已发送 / 未连接丢弃），本地错误为 `"error"`。
    private func emitEvents(_ items: [JSONValue]) -> [JSONValue] {
        items.map { e in
            let name = e["name"]?.stringValue ?? ""
            let sent = try? (e["payload"].map { try client.emitEvent(name, payload: $0) } ?? client.emitEvent(name))
            return sent.map(JSONValue.bool) ?? .string("error")
        }
    }

    /// handler 的 `mutate`（conformance/README.md 2.3）：update 用补丁 API，`nil` 清除。
    private func mutate(_ op: JSONValue) throws {
        let name = op["name"]?.stringValue ?? ""
        switch op["op"]?.stringValue {
        case "register": try register(op["tool"] ?? .null)
        case "busy": client.setBusy(op["value"]?.boolValue ?? false)
        case "declareEvent": try declareEvent(op["event"] ?? .null)
        case "removeEvent": client.removeEvent(name)
        case "update":
            guard case let .object(set)? = op["set"] else { return }
            try tools[name]?.update { d in
                for (key, v) in set { Self.setField(&d, key, v == .null ? nil : v) }
            }
        case "remove": tools.removeValue(forKey: name)?.dispose()
        case "enable": try tools[name]?.setEnabled(true)
        case "disable": try tools[name]?.setEnabled(false)
        default: throw CaseError(description: "未知的 mutate 操作 \(op)")
        }
    }

    /// 字符串数组；nil / 缺省为空数组。
    private static func strings(_ v: JSONValue?) -> [String] {
        (v?.arrayValue ?? []).compactMap(\.stringValue)
    }

    private static func setField(_ d: inout ToolDeclaration, _ key: String, _ v: JSONValue?) {
        switch key {
        case "description": d.description = v?.stringValue ?? ""
        case "inputSchema": d.inputSchema = v.flatMap { try? text($0) }
        case "risk": d.risk = v?.stringValue.map(risk)
        case "activation": d.activation = v?.stringValue.map(activation)
        case "title": d.title = v?.stringValue
        case "annotations": d.annotations = v.map(toolAnnotations)
        case "outputSchema": d.outputSchema = v.flatMap { try? text($0) }
        case "surface": d.surface = v?.stringValue == "view" ? .view : .app
        case "page": d.page = v?.stringValue
        case "backgroundTool": d.backgroundTool = v?.stringValue
        case "concurrency": d.concurrency = Int(v?.doubleValue ?? 0)
        case "exclusive": d.exclusive = v?.stringValue
        case "implements": d.implements = strings(v) // [] = 清除
        case "cache": d.cache = cachePolicy(v) // null = 清除
        case "deprecated": d.deprecated = deprecation(v) // null = 清除
        case "undoable": d.undoable = v?.boolValue ?? false // null / false = 取消声明
        default: break
        }
    }

    /// `app.navigation`（conformance/README.md 2.4）。
    private func navigate(_ pages: JSONValue, _ req: NavigationRequest) throws -> NavigationResult {
        guard let spec = pages[req.page] else { return .failed("未知页面：\(req.page)") }
        for op in spec["mutate"]?.arrayValue ?? [] { try mutate(op) }
        if let msg = spec["throw"]?.stringValue { throw CaseError(description: msg) }
        if let msg = spec["deny"]?.stringValue { return .denied(msg) }
        if let msg = spec["fail"]?.stringValue { return .failed(msg) }
        // 与工具 handler 同一惯用法：抛 userActionRequired（封装层映射为 USER_ACTION_REQUIRED，不是 NAVIGATION_FAILED）
        if let u = spec["userAction"], u != .null {
            throw ToolCallError.userActionRequired(message: u["message"]?.stringValue ?? "", reason: u["reason"]?.stringValue, uri: u["uri"]?.stringValue)
        }
        if spec["failParams"]?.boolValue == true { return .failed(req.paramsJSON ?? "") }
        return .ok
    }

    func registerResource(_ decl: JSONValue) throws {
        let read = decl["read"] ?? .object([:])
        try client.resource(
            decl["name"]?.stringValue ?? "",
            description: decl["description"]?.stringValue ?? "",
            mimeType: decl["mimeType"]?.stringValue,
            realtime: decl["realtime"]?.boolValue ?? false,
            annotations: decl["annotations"].map(contentAnnotations),
            cache: cachePolicy(decl["cache"])
        ) { () async throws -> JSONValue in
            if let v = read["return"] { return v }
            if let f = read["fail"], f != .null {
                throw ToolCallError(f["kind"]?.stringValue ?? ErrorKind.handlerError, f["message"]?.stringValue ?? "", details: f["details"])
            }
            if let u = read["userAction"], u != .null {
                throw ToolCallError.userActionRequired(message: u["message"]?.stringValue ?? "", reason: u["reason"]?.stringValue, uri: u["uri"]?.stringValue)
            }
            throw CaseError(description: read["throw"]?.stringValue ?? "读取失败")
        }
    }
}

// MARK: - 协议取值 → SDK 类型

private func text(_ v: JSONValue) throws -> String {
    String(decoding: try JSONEncoder().encode(v), as: UTF8.self)
}

private func risk(_ s: String) -> Risk {
    let table: [String: Risk] = ["read": .read, "write": .write, "destructive": .destructive, "payment": .payment, "os-sensitive": .osSensitive]
    return table[s] ?? .write
}

private func activation(_ s: String) -> Activation {
    let table: [String: Activation] = ["headless": .headless, "background": .background, "foreground": .foreground]
    return table[s] ?? .foreground
}

private func resultStatus(_ s: String) -> ResultStatus {
    let table: [String: ResultStatus] = ["pending": .pending, "partial": .partial, "noop": .noop]
    return table[s] ?? .done
}

private func toolAnnotations(_ a: JSONValue) -> ToolAnnotations {
    ToolAnnotations(
        title: a["title"]?.stringValue, readOnlyHint: a["readOnlyHint"]?.boolValue,
        destructiveHint: a["destructiveHint"]?.boolValue, idempotentHint: a["idempotentHint"]?.boolValue,
        openWorldHint: a["openWorldHint"]?.boolValue
    )
}

/// `{ttlMs, scope?}` → `CachePolicy`；缺省或 `null` 为 `nil`。
private func cachePolicy(_ v: JSONValue?) -> CachePolicy? {
    guard let v, v != .null, let ttl = v["ttlMs"]?.doubleValue else { return nil }
    let scope: CacheScope? = v["scope"]?.stringValue.map { $0 == "shared" ? .shared : .private }
    return CachePolicy(ttlMs: UInt64(ttl), scope: scope)
}

/// `{message, replacement?, until?}` → `Deprecation`；缺省或 `null` 为 `nil`。
/// `undo` 原样交给 SDK（参数非对象等不合法内容由核心去掉）。
private func undoAction(_ v: JSONValue?) throws -> UndoAction? {
    guard let v, v != .null, let tool = v["tool"]?.stringValue else { return nil }
    return UndoAction(tool: tool, argumentsJson: try v["arguments"].map(text), label: v["label"]?.stringValue)
}

private func deprecation(_ v: JSONValue?) -> Deprecation? {
    guard let v, v != .null, let message = v["message"]?.stringValue else { return nil }
    return Deprecation(message: message, replacement: v["replacement"]?.stringValue, until: v["until"]?.stringValue)
}

private func contentAnnotations(_ a: JSONValue) -> ContentAnnotations {
    ContentAnnotations(
        audience: a["audience"]?.arrayValue?.compactMap { $0.stringValue == "user" ? .user : $0.stringValue == "assistant" ? .assistant : nil },
        priority: a["priority"]?.doubleValue,
        lastModified: a["lastModified"]?.stringValue
    )
}

private extension JSONValue {
    var arrayValue: [JSONValue]? {
        if case let .array(a) = self { return a }
        return nil
    }

    var boolValue: Bool? {
        if case let .bool(b) = self { return b }
        return nil
    }
}
