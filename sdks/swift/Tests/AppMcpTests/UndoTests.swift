@testable import AppMcp
import XCTest

/// 撤销（spec/protocol.md 3.8）：`undoable` 缺省、透传、补丁型 update 与启停不丢；`ToolResult.undo` 到原生结果的转换。不需要 Host。
final class UndoTests: XCTestCase {
    private func makeClient() throws -> AppMcpClient {
        try AppMcpClient(config: AppMcpConfig(appId: "swift-undo", appName: "撤销", hostURL: "ws://127.0.0.1:9"))
    }

    func testUndoableReachesSpec() throws {
        let client = try makeClient()
        defer { client.stop() }
        XCTAssertFalse(try client.tool("u.plain", description: "缺省") { (_: NoArguments, _) in 1 }.specForTest.undoable)
        XCTAssertTrue(try client.tool("u.t", description: "可撤销", undoable: true) { (_: NoArguments, _) in 1 }.specForTest.undoable)
        let bg = try client.backgroundTool("u.bg", description: "后台", undoable: true) { (_: NoArguments, _) in 1 }
        XCTAssertTrue(bg.specForTest.undoable)
    }

    func testUpdateKeepsAndClearsUndoable() throws {
        let client = try makeClient()
        defer { client.stop() }
        let h = try client.tool("u.t", description: "旧", undoable: true) { (_: NoArguments, _) in 1 }
        try h.update(description: "新描述")
        try h.setEnabled(false)
        try h.setEnabled(true)
        XCTAssertTrue(h.specForTest.undoable, "补丁型 update / setEnabled 不得丢失 undoable")
        let with = client.toolsHash
        try h.update { $0.undoable = false }
        XCTAssertFalse(h.specForTest.undoable)
        XCTAssertNotEqual(client.toolsHash, with, "清除后的声明同步到原生层（进 toolsHash）")
        try h.update(undoable: true)
        XCTAssertEqual(client.toolsHash, with)
    }

    func testToolResultUndoToFfi() throws {
        XCTAssertNil(try ToolResult(data: 1).ffi().undo)
        let undo = try UndoAction(tool: "todo.remove", arguments: ["id": 3], label: "删除刚添加的待办")
        XCTAssertEqual(undo, UndoAction(tool: "todo.remove", argumentsJson: #"{"id":3}"#, label: "删除刚添加的待办"))
        XCTAssertEqual(try ToolResult(data: 1, undo: undo).ffi().undo, undo, "完整结果带上 undo")
        let min = UndoAction(tool: "t", argumentsJson: nil, label: nil)
        XCTAssertEqual(try ToolResult(data: 1, undo: min).ffi().undo, min)
    }
}
