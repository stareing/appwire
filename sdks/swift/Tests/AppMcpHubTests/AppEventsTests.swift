import AppMcp
import AppMcpHub
import Foundation
import XCTest

/// 线程安全的收集器（事件回调在 Hub 线程上执行）。
private final class Collected: @unchecked Sendable {
    private let lock = NSLock()
    private var items: [AppEvent] = []
    func add(_ e: AppEvent) { lock.lock(); items.append(e); lock.unlock() }
    var all: [AppEvent] { lock.lock(); defer { lock.unlock() }; return items }
}

private struct Timeout: Error { let what: String }

/// App 事件、订阅与信箱（第 16 项 N3，spec/hub-api.md 3.17）：真实 App 端 emitEvent → Agent 会话 apps.events.subscribe /
/// apps.events 取件、status().events、HubEvent.appEvent 与 setEventHandler。
final class AppEventsTests: XCTestCase {
    private func json(_ text: String?) throws -> [String: Any] {
        try XCTUnwrap(JSONSerialization.jsonObject(with: Data(try XCTUnwrap(text).utf8)) as? [String: Any])
    }

    /// 发出事件（握手完成前返回 false 即丢弃，重试至发出），等待事件流中的下一个 `appEvent`。
    private func emitAndAwait(_ app: AppMcpClient, _ events: AsyncStream<HubEvent>, payload: [String: String]?) async throws -> AppEvent {
        let deadline = Date().addingTimeInterval(10)
        while !(try payload.map { try app.emitEvent("order.shipped", payload: $0) } ?? app.emitEvent("order.shipped")) {
            guard Date() < deadline else { throw Timeout(what: "等待 App 连接") }
            try await Task.sleep(nanoseconds: 20_000_000)
        }
        for await e in events {
            if case let .appEvent(event) = e { return event }
        }
        throw Timeout(what: "事件流已结束")
    }

    func testEmittedEventReachesInboxStatusStreamAndHandler() async throws {
        let hub = try Hub(config: HubConfig(listen: "127.0.0.1:0", enableIpc: false))
        defer { hub.close() }
        let received = Collected()
        hub.setEventHandler { _ in }
        hub.setEventHandler { received.add($0) }
        let events = hub.events()
        let app = try AppMcpClient(config: AppMcpConfig(appId: "shop", appName: "商城", hostURL: "ws://\(hub.listenAddr ?? "")/app"))
        try app.declareEvent("order.shipped", description: "订单已发货")
        app.start()
        defer { app.stop() }

        let sub = try await hub.callTool("apps.events.subscribe", argumentsJSON: #"{"appId":"shop","event":"order.shipped"}"#, session: "s1")
        let subId = try XCTUnwrap(try json(sub.dataJSON)["subscriptionId"] as? String)

        let event = try await emitAndAwait(app, events, payload: ["orderId": "o1"])
        XCTAssertEqual([event.appId, event.name], ["shop", "order.shipped"])
        XCTAssertEqual(try json(event.payloadJson)["orderId"] as? String, "o1")
        XCTAssertTrue(event.id.hasPrefix("ev-") && event.atMs > 0)
        XCTAssertEqual(received.all, [event], "回调收到同一事件")

        let status = try XCTUnwrap(try hub.status().events)
        XCTAssertEqual(status.droppedInvalid, 0)
        XCTAssertEqual(status.subscriptions.count, 1)
        let s = status.subscriptions[0]
        XCTAssertEqual([s.subscriptionId, s.appId, s.event], [subId, "shop", "order.shipped"])
        XCTAssertEqual([s.delivered, s.dropped, s.pending], [1, 0, 1])

        let inbox = try json(try await hub.callTool("apps.events", session: "s1").dataJSON)
        let fetched = try XCTUnwrap(inbox["events"] as? [[String: Any]])
        XCTAssertEqual(fetched.map { $0["name"] as? String }, ["order.shipped"])
        XCTAssertEqual((fetched[0]["payload"] as? [String: Any])?["orderId"] as? String, "o1")
        XCTAssertEqual(inbox["pending"] as? Int, 0)

        // 清除回调后事件流照常，回调不再收到。
        hub.setEventHandler(nil)
        let second = try await emitAndAwait(app, events, payload: nil)
        XCTAssertNil(second.payloadJson)
        XCTAssertEqual(received.all.count, 1)
    }
}
