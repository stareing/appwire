import AppMcp
import AppMcpHub
import Foundation
import XCTest

private struct UndoWaitTimeout: Error {}

private struct Toggle: Codable { var on: Bool }

/// 撤销（第 15 项 X2，spec/hub-api.md 3.23）：真实 App 声明 `undoable` 并在结果中给出 `undo` → `HubTool.undoable`、
/// `CallResult.undo`；`apps.undo` 的结果带 `undoOf`；`status().undo` 有值；不合法的 `undo` 被去掉、调用照常成功。
final class UndoTests: XCTestCase {
    private func awaitTool(_ hub: Hub, _ name: String) async throws -> HubTool {
        let deadline = Date().addingTimeInterval(10)
        while true {
            if let t = hub.tools().first(where: { $0.name == name }) { return t }
            guard Date() < deadline else { throw UndoWaitTimeout() }
            try await Task.sleep(nanoseconds: 20_000_000)
        }
    }

    func testUndoOfferUndoOfAndStatus() async throws {
        let hub = try Hub(config: HubConfig(listen: "127.0.0.1:0", enableIpc: false))
        defer { hub.close() }
        let app = try AppMcpClient(config: AppMcpConfig(appId: "tg", appName: "Toggle", hostURL: "ws://\(hub.listenAddr ?? "")/app"))
        defer { app.stop() }
        try app.backgroundTool("toggle", description: "开关", undoable: true) { (args: Toggle, _) in
            let undo = try UndoAction(tool: "toggle", arguments: Toggle(on: !args.on), label: args.on ? "关掉开关" : "打开开关")
            return ToolResult(data: args, undo: undo)
        }
        try app.backgroundTool("broken", description: "撤销信息不合法") { (_: NoArguments, _) in
            ToolResult(data: ["ok": true], undo: UndoAction(tool: "bad name!", argumentsJson: nil, label: nil))
        }
        app.start()

        let toggleTool = try await awaitTool(hub, "tg.toggle")
        XCTAssertTrue(toggleTool.undoable)
        let brokenTool = try await awaitTool(hub, "tg.broken")
        XCTAssertFalse(brokenTool.undoable)
        let builtin = try await awaitTool(hub, "apps.list")
        XCTAssertFalse(builtin.undoable, "内置工具为 false")

        let first = try await hub.callTool("tg.toggle", argumentsJSON: #"{"on":true}"#)
        XCTAssertNil(first.error)
        let offer = try XCTUnwrap(first.undo, "已登记撤销")
        XCTAssertEqual(offer.label, "关掉开关")
        XCTAssertTrue((1...1_800_000).contains(offer.expiresInMs), "\(offer)")
        XCTAssertNil(first.undoOf)
        XCTAssertEqual(try hub.status().undo, UndoStatus(ttlMs: 1_800_000, maxPerTask: 32, records: 1))

        let undone = try await hub.callTool("apps.undo", argumentsJSON: "{}")
        XCTAssertNil(undone.error)
        XCTAssertEqual(undone.undoOf, first.callId)
        XCTAssertEqual(try undone.decode(Toggle.self).on, false, "逆调用以相反值执行")
        XCTAssertEqual(undone.undo?.label, "打开开关", "逆调用结果再次登记（重做）")
        let again = try await hub.callTool("apps.undo", argumentsJSON: #"{"callId":"\#(first.callId)"}"#)
        XCTAssertEqual(again.error?.kind, "TOOL_NOT_FOUND", "只能撤销一次")

        let bad = try await hub.callTool("tg.broken")
        XCTAssertNil(bad.error)
        XCTAssertNil(bad.undo, "不合法的 undo 被核心去掉")
        XCTAssertEqual(bad.dataJSON, #"{"ok":true}"#)
    }
}
