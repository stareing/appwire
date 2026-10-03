@testable import AppMcp
import XCTest

/// 事件（spec/protocol.md 3.5）的本地行为：未连接时丢弃、本地错误；不需要 Host。连接后的发送见一致性用例 event-emit。
final class EventsTests: XCTestCase {
    private func makeClient() throws -> AppMcpClient {
        try AppMcpClient(config: AppMcpConfig(appId: "swift-unit", appName: "事件", hostURL: "ws://127.0.0.1:9"))
    }

    private func assertError(_ body: () throws -> Bool, _ check: (AppMcpError) -> Bool, line: UInt = #line) {
        XCTAssertThrowsError(try body(), line: line) { e in
            XCTAssertTrue((e as? AppMcpError).map(check) ?? false, "\(e)", line: line)
        }
    }

    func testEmitWhileDisconnectedIsDropped() throws {
        let client = try makeClient()
        try client.declareEvent("order.shipped", description: "订单已发货", payloadSchema: #"{"type":"object"}"#)
        XCTAssertFalse(try client.emitEvent("order.shipped", payload: ["orderId": "o1"]))
        XCTAssertFalse(try client.emitEvent("order.shipped", payloadJSON: #"{"orderId":"o1"}"#))
        XCTAssertFalse(try client.emitEvent("order.shipped"))
    }

    func testLocalErrors() throws {
        let client = try makeClient()
        try client.declareEvent("order.shipped", description: "订单已发货")
        let invalidName: (AppMcpError) -> Bool = { if case .InvalidName = $0 { return true } else { return false } }
        let invalidJSON: (AppMcpError) -> Bool = { if case .InvalidJson = $0 { return true } else { return false } }
        assertError({ try client.emitEvent("nope") }, invalidName)
        assertError({ try client.emitEvent("order.shipped", payload: 1) }, invalidJSON)
        assertError({ try client.emitEvent("order.shipped", payload: [1]) }, invalidJSON)
        assertError({ try client.emitEvent("order.shipped", payloadJSON: "{") }, invalidJSON)
        assertError({ try client.emitEvent("order.shipped", payload: ["blob": String(repeating: "x", count: 9000)]) }, invalidJSON)
        assertError({ try client.emitEvent("order.shipped", payload: ["x": Double.nan]) }, invalidJSON)
        assertError({ try client.declareEvent("bad.schema", description: "", payloadSchema: "{"); return true }, invalidJSON)
        assertError({ try client.declareEvent("坏 名", description: ""); return true }, invalidName)
    }

    func testRemoveEvent() throws {
        let client = try makeClient()
        try client.declareEvent("download.done", description: "下载完成")
        XCTAssertTrue(client.removeEvent("download.done"))
        XCTAssertFalse(client.removeEvent("download.done"))
        XCTAssertThrowsError(try client.emitEvent("download.done"))
    }
}
