import AppMcp
import AppMcpHub
import Foundation
import XCTest

private struct IntentsTimeout: Error {}

/// 标准意图（第 16 项 N4，spec/intents.md 第 4 节）：真实 App 以 `implements` 声明 → `HubTool.implements`、Agent 会话
/// `apps.intents`、`setIntentDefaults` / `intents()` 与 `status().intents`。
final class IntentsTests: XCTestCase {
    private let schema = #"{"type":"object","properties":{"to":{"type":"array"},"text":{"type":"string"}}}"#

    /// Agent 会话中 `apps.intents {intent: "message.send"}` 的实现者（"工具全名=是否默认"）。
    private func implementations(_ hub: Hub) async throws -> [String] {
        let out = try await hub.callTool("apps.intents", argumentsJSON: #"{"intent":"message.send"}"#, session: "agent")
        XCTAssertNil(out.error)
        let data = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(try XCTUnwrap(out.dataJSON).utf8)) as? [String: Any])
        let intents = try XCTUnwrap(data["intents"] as? [[String: Any]])
        let entry = try XCTUnwrap(intents.first { $0["intent"] as? String == "message.send@1" })
        XCTAssertEqual(entry["known"] as? Bool, true)
        let list = try XCTUnwrap(entry["implementations"] as? [[String: Any]])
        return list.map { "\($0["tool"] as? String ?? "")=\($0["default"] as? Bool ?? false)" }
    }

    func testImplementsListedAndDefaultsReorder() async throws {
        let hub = try Hub(config: HubConfig(listen: "127.0.0.1:0", enableIpc: false))
        defer { hub.close() }
        let app = try AppMcpClient(config: AppMcpConfig(appId: "mail", appName: "邮件", hostURL: "ws://\(hub.listenAddr ?? "")/app"))
        for name in ["a_send", "b_send"] {
            try app.backgroundTool(name, description: "发邮件", inputSchema: schema, implements: ["message.send@1"]) { (_: JSONValue, _) in 1 }
        }
        app.start()
        defer { app.stop() }

        let filter = ToolFilter(apps: ["mail"])
        // 连接之后工具列表才送达：等到出现。
        let deadline = Date().addingTimeInterval(10)
        while !hub.tools(filter).contains(where: { $0.name == "mail.b_send" }) {
            guard Date() < deadline else { throw IntentsTimeout() }
            try await Task.sleep(nanoseconds: 20_000_000)
        }
        let b = try XCTUnwrap(hub.tools(filter).first { $0.name == "mail.b_send" })
        XCTAssertEqual(b.implements, ["message.send@1"], "HubTool.implements 透传")
        XCTAssertEqual(try hub.intents(), IntentsStatus(defaults: [:], lastError: nil))
        let unset = try await implementations(hub)
        XCTAssertEqual(unset, ["mail.a_send=false", "mail.b_send=false"], "无默认时按全名排序")

        try hub.setIntentDefaults(["message.send": "mail.b_send"])
        let ordered = try await implementations(hub)
        XCTAssertEqual(ordered, ["mail.b_send=true", "mail.a_send=false"], "默认排首位")
        let st = try hub.intents()
        XCTAssertEqual(st.defaults, ["message.send": "mail.b_send"])
        XCTAssertNil(st.lastError)
        XCTAssertEqual(try hub.status().intents, st)

        XCTAssertThrowsError(try hub.setIntentDefaults(["Bad Verb": "mail.a_send"])) { error in
            guard case let HubError.Tool(kind, _, _) = error else { return XCTFail("\(error)") }
            XCTAssertEqual(kind, "INVALID_INPUT")
        }
        let after = try hub.intents()
        XCTAssertEqual(after.defaults, ["message.send": "mail.b_send"], "旧值保留")
        XCTAssertTrue(after.lastError?.contains("Bad Verb") == true, "\(after)")

        try hub.setIntentDefaults([:])
        XCTAssertEqual(try hub.intents(), IntentsStatus(defaults: [:], lastError: nil))
    }

    /// `HubConfig.intentDefaults` 经绑定生效；不合法时 Hub 照常启动、空表并记下原因。
    func testConfigIntentDefaults() throws {
        XCTAssertNil(HubConfig().intentDefaults)
        let good = try Hub(config: HubConfig(enableListen: false, enableIpc: false, intentDefaults: ["link.open@1": "web.open"]))
        XCTAssertEqual(try good.intents().defaults, ["link.open@1": "web.open"])
        good.close()
        let bad = try Hub(config: HubConfig(enableListen: false, enableIpc: false, intentDefaults: ["link.open": "nodot"]))
        defer { bad.close() }
        let st = try bad.intents()
        XCTAssertTrue(st.defaults.isEmpty && st.lastError?.contains("nodot") == true, "\(st)")
    }
}
