@testable import AppMcp
import XCTest

final class UnitTests: XCTestCase {
    func testErrorKindsMatchNative() {
        let declared: Set<String> = [
            ErrorKind.toolNotFound, ErrorKind.toolDisabled, ErrorKind.invalidInput, ErrorKind.userRejected,
            ErrorKind.timeout, ErrorKind.handlerError, ErrorKind.cancelled, ErrorKind.appDisconnected,
            ErrorKind.appNotInstalled, ErrorKind.launchFailed, ErrorKind.appNotResponding,
            ErrorKind.instanceFrozen, ErrorKind.resourceNotFound, ErrorKind.unauthorized,
            ErrorKind.unsupportedProtocol, ErrorKind.rateLimited, ErrorKind.payloadTooLarge,
            ErrorKind.policyDenied, ErrorKind.userActionRequired, ErrorKind.navigationFailed, ErrorKind.navigationDenied,
            ErrorKind.locked,
        ]
        XCTAssertEqual(ErrorKind.all, declared)
    }

    func testUserActionRequiredDetails() {
        let e = ToolCallError.userActionRequired(message: "请先登录", reason: UserActionReason.login, uri: "shop://login")
        XCTAssertEqual(e.kind, ErrorKind.userActionRequired)
        XCTAssertEqual(e.message, "请先登录")
        XCTAssertEqual(e.details, .object(["reason": .string("login"), "uri": .string("shop://login")]))
        XCTAssertEqual(ToolCallError.userActionRequired(message: "x", reason: "custom").details, .object(["reason": .string("custom")]))
        XCTAssertNil(ToolCallError.userActionRequired(message: "切到前台").details)
    }

    func testJSONValueRoundTrip() throws {
        let text = #"{"a":[1,"x",true,null],"b":{"c":2.5}}"#
        let v = try decodeJSON(JSONValue.self, text)
        XCTAssertEqual(v["b"]?["c"]?.doubleValue, 2.5)
        XCTAssertEqual(try encodeJSON(v), text)
    }

    func testNoArgumentsDecodesEmptyObject() throws {
        XCTAssertEqual(try decodeJSON(NoArguments.self, ""), NoArguments())
        XCTAssertEqual(try decodeJSON(NoArguments.self, #"{"extra":1}"#), NoArguments())
    }

    func testGateAllowsOnlyOneOutcome() {
        let g = Gate()
        XCTAssertTrue(g.begin())
        XCTAssertFalse(g.abandon())
        let g2 = Gate()
        XCTAssertTrue(g2.abandon())
        XCTAssertFalse(g2.begin())
    }

    func testLaunchMapsErrors() async {
        final class Box: @unchecked Sendable {
            let lock = NSLock()
            var value: (String, String, String?)?
            func set(_ v: (String, String, String?)) { lock.lock(); value = v; lock.unlock() }
        }
        let box = Box()
        await launch(on: .background, timeout: nil, fail: { box.set(($0, $1, $2)) }) {
            throw ToolCallError(ErrorKind.userRejected, "不行")
        }.value
        XCTAssertEqual(box.value?.0, ErrorKind.userRejected)
        XCTAssertNil(box.value?.2)

        await launch(on: .background, timeout: nil, fail: { box.set(($0, $1, $2)) }) {
            throw ToolCallError(ErrorKind.invalidInput, "库存不足", details: .object(["available": .number(2)]))
        }.value
        XCTAssertEqual(box.value?.2, #"{"available":2}"#)

        await launch(on: .background, timeout: nil, fail: { box.set(($0, $1, $2)) }) {
            _ = try decodeJSON(Int.self, "\"x\"")
        }.value
        XCTAssertEqual(box.value?.0, ErrorKind.invalidInput)

        let t = launch(on: .background, timeout: nil, fail: { box.set(($0, $1, $2)) }) {
            try await Task.sleep(nanoseconds: 5_000_000_000)
        }
        t.cancel()
        await t.value
        XCTAssertEqual(box.value?.0, ErrorKind.cancelled)
    }

    func testImplementsReachSpec() throws {
        let client = try AppMcpClient(config: AppMcpConfig(appId: "swift-unit", appName: "Swift 单元测试", hostURL: "ws://127.0.0.1:9"))
        defer { client.stop() }
        XCTAssertEqual(try client.tool("i.plain", description: "缺省") { (_: NoArguments, _) in }.specForTest.implements, [])
        let h = try client.tool("i.send", description: "发消息", implements: ["message.send@1"]) { (_: NoArguments, _) in }
        XCTAssertEqual(h.specForTest.implements, ["message.send@1"])
        try h.update(description: "新")
        try h.setEnabled(false)
        XCTAssertEqual(h.specForTest.implements, ["message.send@1"], "补丁型 update 不得重置意图声明")
        try h.update { $0.implements = [] }
        XCTAssertEqual(h.specForTest.implements, [])
        try h.update(implements: ["link.open@1"])
        XCTAssertEqual(h.specForTest.implements, ["link.open@1"])
        let bg = try client.backgroundTool("i.bg", description: "后台", implements: ["link.open@1"]) { (_: NoArguments, _) in 1 }
        XCTAssertEqual(bg.specForTest.implements, ["link.open@1"])
        XCTAssertThrowsError(try client.tool("i.bad", description: "缺版本", implements: ["message.send"]) { (_: NoArguments, _) in }) { err in
            guard case AppMcpError.InvalidName = err else { return XCTFail("期望 InvalidName，得到 \(err)") }
        }
        XCTAssertThrowsError(try h.update(implements: ["Bad Verb@1"]))
    }

    func testRegistrationErrors() throws {
        let client = try AppMcpClient(config: AppMcpConfig(appId: "swift-unit", appName: "Swift 单元测试", hostURL: "ws://127.0.0.1:9"))
        defer { client.stop() }
        let h = try client.tool("cart.add", description: "加入购物车") { (_: NoArguments, _) in 1 }
        XCTAssertEqual(h.name, "cart.add")
        XCTAssertThrowsError(try client.tool("cart.add", description: "重复") { (_: NoArguments, _) in 1 }) { err in
            guard case AppMcpError.DuplicateName = err else { return XCTFail("期望 DuplicateName，得到 \(err)") }
        }
        XCTAssertThrowsError(try client.tool("bad name!", description: "非法") { (_: NoArguments, _) in 1 })
        try h.setEnabled(false)
        try h.update(description: "新描述")
        h.dispose()
        try client.tool("cart.add", description: "注销后可重新注册") { (_: NoArguments, _) in }

        let edit = try client.tool("doc.edit", description: "改文档", concurrency: 1, exclusive: "doc") { (_: NoArguments, _) in }
        try edit.update(concurrency: 2, exclusive: "doc2")
        try edit.update { $0.exclusive = nil }
        try client.backgroundTool("doc.save", description: "保存", exclusive: "doc") { (_: NoArguments, _) in 1 }
        XCTAssertThrowsError(try client.tool("doc.bad", description: "非法组名", exclusive: "bad group") { (_: NoArguments, _) in 1 }) { err in
            guard case AppMcpError.InvalidName = err else { return XCTFail("期望 InvalidName，得到 \(err)") }
        }

        let scope = try client.scope("page")
        try scope.tool("page.one", description: "页内工具") { (_: NoArguments, _) in "x" }
        scope.dispose()
        try client.tool("page.one", description: "scope 注销后可重新注册") { (_: NoArguments, _) in "y" }

        let r = try client.resource("cart", description: "购物车") { ["items": [String]()] }
        try r.notifyChanged()
        XCTAssertEqual(client.state.status, .idle)
        XCTAssertFalse(client.instanceId.isEmpty)
    }

    func testStoppedClientRejectsRegistration() throws {
        let client = try AppMcpClient(config: AppMcpConfig(appId: "swift-unit", appName: "停止", overview: AppOverview(summary: "测试")))
        client.stop()
        XCTAssertThrowsError(try client.tool("x", description: "x") { (_: NoArguments, _) in 1 }) { err in
            guard case AppMcpError.Stopped = err else { return XCTFail("期望 Stopped，得到 \(err)") }
        }
    }
}
