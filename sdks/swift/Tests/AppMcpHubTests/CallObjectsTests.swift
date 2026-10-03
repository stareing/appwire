import AppMcp
import AppMcpHub
import Foundation
import XCTest

private struct NoArgs: Codable, Sendable {}

/// 调用对象（第 16 项 P5，spec/hub-api.md 3.6「调用对象」）：HubStatus.calls、内置工具 apps.calls / apps.cancel。
final class CallObjectsTests: XCTestCase {
    private func json(_ text: String?) throws -> [String: Any] {
        try XCTUnwrap(JSONSerialization.jsonObject(with: Data(try XCTUnwrap(text).utf8)) as? [String: Any])
    }

    /// 慢调用进行中：status().calls 含该调用（running、进度）；apps.calls 只见自己的；apps.cancel 后发起方得到 CANCELLED。
    func testInflightCallVisibleAndCancellable() async throws {
        let hub = try Hub(config: HubConfig(listen: "127.0.0.1:0", enableIpc: false))
        defer { hub.close() }
        XCTAssertTrue(Set(hub.tools().map(\.name)).isSuperset(of: ["apps.calls", "apps.cancel"]))
        let app = try AppMcpClient(config: AppMcpConfig(appId: "jobs", appName: "作业", hostURL: "ws://\(hub.listenAddr ?? "")/app"))
        try app.tool("job.run", description: "慢作业") { (_: NoArgs, ctx: ToolContext) in
            ctx.progress(1, total: 4, message: "第一步")
            let deadline = Date().addingTimeInterval(10)
            while ctx.cancelReason == nil && Date() < deadline { try await Task.sleep(nanoseconds: 20_000_000) }
            return ["ok": true]
        }
        app.start()
        defer { app.stop() }

        var deadline = Date().addingTimeInterval(10)
        while hub.tools(ToolFilter(apps: ["jobs"], includeBuiltin: false)).filter({ $0.availability == .available }).isEmpty {
            guard Date() < deadline else { return XCTFail("等待工具注册超时") }
            try await Task.sleep(nanoseconds: 20_000_000)
        }
        let slow = Task { try await hub.callTool("jobs.job.run", session: "s1", callId: "slow-1") }
        var found: CallStatus?
        deadline = Date().addingTimeInterval(10)
        while true {
            found = try hub.status().calls?.first { $0.callId == "slow-1" }
            if let c = found, c.state == CallState.running, c.progress != nil { break }
            guard Date() < deadline else { return XCTFail("等待调用进入 running 超时") }
            try await Task.sleep(nanoseconds: 20_000_000)
        }
        let call = try XCTUnwrap(found)
        XCTAssertEqual([call.name, call.caller, call.subject], ["jobs.job.run", "api:s1", "api"])
        XCTAssertFalse((call.instanceId ?? "").isEmpty)
        XCTAssertEqual(call.progress, 1)
        XCTAssertEqual(call.progressTotal, 4)
        XCTAssertEqual(call.progressMessage, "第一步")

        let ownOut = try await hub.callTool("apps.calls", session: "s1")
        let own = try XCTUnwrap(try json(ownOut.dataJSON)["calls"] as? [[String: Any]])
        XCTAssertEqual(own.map { $0["callId"] as? String }, ["slow-1"])
        XCTAssertNil(own[0]["caller"])
        let otherOut = try await hub.callTool("apps.calls", session: "s2")
        let other = try XCTUnwrap(try json(otherOut.dataJSON)["calls"] as? [Any])
        XCTAssertTrue(other.isEmpty)
        let args = #"{"callId":"slow-1"}"#
        let foreign = try await hub.callTool("apps.cancel", argumentsJSON: args, session: "s2")
        XCTAssertEqual(foreign.error?.kind, "TOOL_NOT_FOUND")

        let cancelled = try await hub.callTool("apps.cancel", argumentsJSON: args, session: "s1")
        XCTAssertEqual(try json(cancelled.dataJSON)["cancelled"] as? Bool, true)
        let out = try await slow.value
        XCTAssertEqual(out.error?.kind, "CANCELLED")
        XCTAssertFalse(try hub.status().calls?.contains { $0.callId == "slow-1" } ?? true)
    }
}
