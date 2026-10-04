import AppMcp
import AppMcpHub
import Foundation
import XCTest

private struct SchemaWaitTimeout: Error {}

/// 工具演进（第 16 项 O4，spec/hub-api.md 3.21）：真实 App 声明弃用工具 → `HubTool.deprecated` 原样、`schemaHash` 有值；
/// schema 变化后 `schemaHash` 变化，破坏性变化记入 `status().schemaChanges`。
final class SchemaEvolutionTests: XCTestCase {
    private func awaitTool(_ hub: Hub, _ name: String, where pred: (HubTool) -> Bool = { _ in true }) async throws -> HubTool {
        let deadline = Date().addingTimeInterval(10)
        while true {
            if let t = hub.tools().first(where: { $0.name == name && pred($0) }) { return t }
            guard Date() < deadline else { throw SchemaWaitTimeout() }
            try await Task.sleep(nanoseconds: 20_000_000)
        }
    }

    private func schema(required: Bool) -> String {
        #"{"type":"object","properties":{"q":{"type":"string"}}"# + (required ? #","required":["q"]}"# : "}")
    }

    func testDeprecatedToolAndSchemaHash() async throws {
        let hub = try Hub(config: HubConfig(listen: "127.0.0.1:0", enableIpc: false))
        defer { hub.close() }
        let app = try AppMcpClient(config: AppMcpConfig(appId: "lib", appName: "Lib", hostURL: "ws://\(hub.listenAddr ?? "")/app"))
        defer { app.stop() }
        let old = try app.backgroundTool(
            "q.old", description: "旧版查询", inputSchema: schema(required: false),
            deprecated: .init(message: "改用 q.new", replacement: "q.new", until: "2027-06-30")
        ) { (_: NoArguments, _) in 1 }
        try app.backgroundTool("q.new", description: "新版查询") { (_: NoArguments, _) in 2 }
        app.start()

        let t = try await awaitTool(hub, "lib.q.old")
        XCTAssertEqual(t.deprecated?.message, "改用 q.new")
        XCTAssertEqual(t.deprecated?.replacement, "q.new")
        XCTAssertEqual(t.deprecated?.until, "2027-06-30")
        let hash = try XCTUnwrap(t.schemaHash)
        XCTAssertEqual(hash.count, 16, hash)
        let fresh = try await awaitTool(hub, "lib.q.new")
        XCTAssertNil(fresh.deprecated, "未弃用")
        XCTAssertNotNil(fresh.schemaHash)
        let builtin = try await awaitTool(hub, "apps.list")
        XCTAssertNil(builtin.schemaHash, "内置工具不带")
        XCTAssertNil(builtin.deprecated)

        try old.update(inputSchema: schema(required: true))
        let changed = try await awaitTool(hub, "lib.q.old") { $0.schemaHash != hash }
        XCTAssertEqual(changed.deprecated, t.deprecated, "更新 schema 不丢弃用声明")
        let rec = try XCTUnwrap(try hub.status().schemaChanges?.first { $0.appId == "lib" && $0.tool == "q.old" }, "记入 schemaChanges")
        XCTAssertEqual(rec.level, .breaking)
        XCTAssertFalse(rec.changes.isEmpty)
        XCTAssertGreaterThan(rec.at, 0)
    }
}
