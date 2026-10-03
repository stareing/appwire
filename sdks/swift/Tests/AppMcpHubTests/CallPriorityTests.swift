import AppMcp
import AppMcpHub
import Foundation
import XCTest

/// 按开始执行的顺序记录调用标签。
private final class StartOrder: @unchecked Sendable {
    private let lock = NSLock()
    private var tags: [String] = []
    func add(_ tag: String) { lock.lock(); tags.append(tag); lock.unlock() }
    var all: [String] { lock.lock(); defer { lock.unlock() }; return tags }
}

private struct JobArgs: Codable, Sendable {
    var tag: String?
    var delayMs: Int?
}

/// 第 16 项 P6（spec/hub-api.md 3.15）：`callTool(priority:)` 经 tools/invoke 到达 App，调用队列先交互、后后台。
final class CallPriorityTests: XCTestCase {
    func testPriorityReachesAppQueue() async throws {
        let hub = try Hub(config: HubConfig(listen: "127.0.0.1:0", enableIpc: false))
        defer { hub.close() }
        let app = try AppMcpClient(config: AppMcpConfig(
            appId: "jobs", appName: "作业", hostURL: "ws://\(hub.listenAddr ?? "")/app", maxConcurrentCalls: 1
        ))
        let started = StartOrder()
        try app.tool("job.run", description: "执行") { (args: JobArgs, _) in
            if let tag = args.tag { started.add(tag) }
            if let ms = args.delayMs, ms > 0 { try await Task.sleep(nanoseconds: UInt64(ms) * 1_000_000) }
            return ["ok": true]
        }
        app.start()
        defer { app.stop() }

        let deadline = Date().addingTimeInterval(10)
        while hub.tools(ToolFilter(apps: ["jobs"], includeBuiltin: false)).filter({ $0.availability == .available }).isEmpty {
            guard Date() < deadline else { return XCTFail("等待工具注册超时") }
            try await Task.sleep(nanoseconds: 20_000_000)
        }
        // 预热：首次调用的总览附带等不计入排队顺序
        let warm = try await hub.callTool("jobs.job.run")
        XCTAssertNil(warm.error)

        @Sendable func call(_ tag: String, _ priority: CallPriority, _ delayMs: Int) -> Task<CallResult, Error> {
            Task { try await hub.callTool("jobs.job.run", arguments: JobArgs(tag: tag, delayMs: delayMs), priority: priority) }
        }
        let slow = call("slow", .normal, 600)
        try await Task.sleep(nanoseconds: 200_000_000)
        let background = call("background", .background, 0)
        try await Task.sleep(nanoseconds: 50_000_000)
        let interactive = call("interactive", .interactive, 0)
        for task in [slow, background, interactive] {
            let out = try await task.value
            XCTAssertNil(out.error)
        }
        XCTAssertEqual(started.all, ["slow", "interactive", "background"])
    }
}
