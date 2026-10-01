import AppMcp
import Foundation
import XCTest

struct AddArgs: Codable, Sendable {
    let a: Int
    let b: Int
}

struct Greeting: Codable, Sendable {
    let name: String
}

/// 启动 crates/native 的 fake_host，经真实 WebSocket 驱动工具调用。需要 cargo。
final class IntegrationTests: XCTestCase {
    private var repoRoot: URL {
        URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent()
    }

    private var targetDir: String {
        // 仓库根目录下的 target/（与 cargo 默认一致）
        ProcessInfo.processInfo.environment["CARGO_TARGET_DIR"] ?? repoRoot.appendingPathComponent("target").path
    }

    private func fakeHost() throws -> String {
        if let explicit = ProcessInfo.processInfo.environment["APP_MCP_FAKE_HOST"] { return explicit }
        let build = Process()
        build.executableURL = URL(fileURLWithPath: "/usr/bin/env")
        build.arguments = ["cargo", "build", "-q", "-p", "app-mcp-native", "--example", "fake_host"]
        build.currentDirectoryURL = repoRoot
        var env = ProcessInfo.processInfo.environment
        env["CARGO_TARGET_DIR"] = targetDir
        build.environment = env
        do {
            try build.run()
        } catch {
            throw XCTSkip("没有 cargo，跳过集成测试：\(error)")
        }
        build.waitUntilExit()
        guard build.terminationStatus == 0 else { throw XCTSkip("fake_host 构建失败") }
        return targetDir + "/debug/examples/fake_host"
    }

    private func readLine(_ handle: FileHandle) -> String {
        var bytes = Data()
        while true {
            let b = handle.readData(ofLength: 1)
            if b.isEmpty || b == Data([0x0A]) { break }
            bytes.append(b)
        }
        return String(decoding: bytes, as: UTF8.self)
    }

    func testInvokeToolsViaFakeHost() async throws {
        let host = Process()
        host.executableURL = URL(fileURLWithPath: try fakeHost())
        host.arguments = [
            "--invoke", "math.add", "--args", #"{"a":2,"b":40}"#,
            "--invoke", "greet", "--args", #"{"name":"世界"}"#,
            "--invoke", "cart.checkout",
            "--invoke", "boom",
            "--invoke", "bg.echo", "--args", #"{"name":"后台"}"#,
            "--read", "cart",
            "--timeout-ms", "15000",
        ]
        let out = Pipe()
        host.standardOutput = out
        try host.run()
        defer { if host.isRunning { host.terminate() } }

        let first = readLine(out.fileHandleForReading)
        XCTAssertTrue(first.hasPrefix("LISTENING "), first)
        let addr = String(first.dropFirst("LISTENING ".count))

        let client = try AppMcpClient(config: AppMcpConfig(appId: "swift-it", appName: "Swift 集成测试", hostURL: "ws://\(addr)"))
        try client.tool("math.add", description: "两数相加", risk: .read) { (args: AddArgs, ctx) in
            dispatchPrecondition(condition: .onQueue(.main))
            ctx.addStateHint("cart")
            ctx.progress(1, total: 2, message: "相加")
            return ["sum": args.a + args.b]
        }
        try client.tool("greet", description: "问候") { (g: Greeting, _) in "你好，\(g.name)" }
        try client.tool("cart.checkout", description: "结账", risk: .payment) { (_: NoArguments, _) in
            throw ToolCallError(ErrorKind.userRejected, "用户取消了结账")
        }
        struct Boom: Error {}
        try client.tool("boom", description: "抛异常") { (_: NoArguments, _) in throw Boom() }
        try client.backgroundTool("bg.echo", description: "后台回显") { (g: Greeting, _) in g.name }
        try client.resource("cart", description: "购物车") { ["items": ["A"]] }
        client.start()

        let handle = out.fileHandleForReading
        let data = await Task.detached { handle.readDataToEndOfFile() }.value
        host.waitUntilExit()
        client.stop()
        let text = String(decoding: data, as: UTF8.self)
        XCTAssertEqual(host.terminationStatus, 0, text)

        let lines = try text.split(separator: "\n").map {
            try JSONSerialization.jsonObject(with: Data($0.utf8)) as! [String: Any]
        }
        XCTAssertEqual(lines.first?["type"] as? String, "tools")
        var results: [String: [String: Any]] = [:]
        for l in lines.dropFirst() where l["type"] as? String != "progress" { results[l["name"] as! String] = l }
        let progress = lines.filter { $0["type"] as? String == "progress" }
        XCTAssertEqual(progress.count, 1)
        XCTAssertEqual(progress.first?["progress"] as? Double, 1)
        XCTAssertEqual(progress.first?["total"] as? Double, 2)
        XCTAssertEqual(progress.first?["message"] as? String, "相加")

        let add = results["math.add"]?["result"] as? [String: Any]
        XCTAssertEqual((add?["data"] as? [String: Any])?["sum"] as? Int, 42)
        XCTAssertEqual(add?["stateHints"] as? [String], ["cart"])
        XCTAssertEqual((results["greet"]?["result"] as? [String: Any])?["data"] as? String, "你好，世界")
        let rejected = results["cart.checkout"]?["error"] as? [String: Any]
        XCTAssertEqual((rejected?["data"] as? [String: Any])?["kind"] as? String, "USER_REJECTED")
        XCTAssertEqual(rejected?["message"] as? String, "用户取消了结账")
        let boom = results["boom"]?["error"] as? [String: Any]
        XCTAssertEqual((boom?["data"] as? [String: Any])?["kind"] as? String, "HANDLER_ERROR")
        XCTAssertEqual((results["bg.echo"]?["result"] as? [String: Any])?["data"] as? String, "后台")
        let cart = (results["cart"]?["result"] as? [String: Any])?["contents"] as? [String: Any]
        XCTAssertEqual(cart?["items"] as? [String], ["A"])
    }

    /// 工具注解 + outputSchema 到达 Host；结构化结果（pending + stateResource + summary + 内容标注）原样回给 Host；普通返回值不变。
    func testToolOptionsAndStructuredResultReachHost() async throws {
        let host = Process()
        host.executableURL = URL(fileURLWithPath: try fakeHost())
        host.arguments = [
            "--tool-info",
            "--invoke", "order.submit",
            "--invoke", "bg.submit",
            "--invoke", "plain",
            "--timeout-ms", "15000",
        ]
        let out = Pipe()
        host.standardOutput = out
        try host.run()
        defer { if host.isRunning { host.terminate() } }

        let first = readLine(out.fileHandleForReading)
        XCTAssertTrue(first.hasPrefix("LISTENING "), first)
        let addr = String(first.dropFirst("LISTENING ".count))

        let client = try AppMcpClient(config: AppMcpConfig(appId: "swift-it", appName: "Swift 集成测试", hostURL: "ws://\(addr)"))
        let schema = #"{"type":"object","properties":{"orderId":{"type":"string"}}}"#
        try client.tool(
            "order.submit", description: "下单",
            annotations: ToolAnnotations(idempotentHint: false, openWorldHint: true),
            outputSchema: schema
        ) { (_: NoArguments, ctx) in
            ctx.addStateHint("cart")
            return ToolResult(
                data: ["orderId": "o1"],
                status: .pending,
                stateResource: "order.state",
                summary: "已提交，等待用户在 App 内付款",
                annotations: ContentAnnotations(audience: [.user], priority: 0.5)
            )
        }
        try client.backgroundTool("bg.submit", description: "后台部分完成") { (_: NoArguments, _) in
            ToolResult<String>(data: nil, status: .partial, summary: "完成 2 / 3")
        }
        try client.tool("plain", description: "普通返回值", risk: .read) { (_: NoArguments, _) in ["ok": true] }
        client.start()

        let handle = out.fileHandleForReading
        let data = await Task.detached { handle.readDataToEndOfFile() }.value
        host.waitUntilExit()
        client.stop()
        let text = String(decoding: data, as: UTF8.self)
        XCTAssertEqual(host.terminationStatus, 0, text)

        let lines = try text.split(separator: "\n").map {
            try JSONSerialization.jsonObject(with: Data($0.utf8)) as! [String: Any]
        }
        let info = lines.first?["toolInfo"] as? [String: Any]
        let submit = info?["order.submit"] as? NSDictionary
        XCTAssertEqual(submit, [
            "risk": "write",
            "annotations": ["idempotentHint": false, "openWorldHint": true],
            "outputSchema": ["type": "object", "properties": ["orderId": ["type": "string"]]],
        ] as NSDictionary)
        XCTAssertEqual(info?["plain"] as? NSDictionary, ["risk": "read"] as NSDictionary)

        var results: [String: [String: Any]] = [:]
        for l in lines.dropFirst() { results[l["name"] as! String] = l }
        XCTAssertEqual(results["order.submit"]?["result"] as? NSDictionary, [
            "data": ["orderId": "o1"], "stateHints": ["cart"], "status": "pending", "stateResource": "order.state",
            "summary": "已提交，等待用户在 App 内付款", "annotations": ["audience": ["user"], "priority": 0.5],
        ] as NSDictionary)
        XCTAssertEqual(results["bg.submit"]?["result"] as? NSDictionary, [
            "data": NSNull(), "status": "partial", "summary": "完成 2 / 3",
        ] as NSDictionary)
        XCTAssertEqual(results["plain"]?["result"] as? NSDictionary, ["data": ["ok": true]] as NSDictionary)
    }

    /// idle 休眠 → handleWake 回连（toolsCurrent 跳过同步）→ 调用（含 details 错误、ctx.hold）→ 再休眠。
    func testIdleSleepWakeRoundTrip() async throws {
        let host = Process()
        host.executableURL = URL(fileURLWithPath: try fakeHost())
        host.arguments = [
            "--await-sleep", "--wake",
            "--invoke", "echo", "--args", #"{"name":"醒了"}"#,
            "--invoke", "stock.reserve",
            "--await-sleep",
            "--timeout-ms", "20000",
        ]
        let out = Pipe()
        host.standardOutput = out
        try host.run()
        defer { if host.isRunning { host.terminate() } }
        let handle = out.fileHandleForReading

        let first = readLine(handle)
        XCTAssertTrue(first.hasPrefix("LISTENING "), first)
        let addr = String(first.dropFirst("LISTENING ".count))

        let idleExit = expectation(description: "onIdleExit")
        idleExit.assertForOverFulfill = false
        let client = try AppMcpClient(config: AppMcpConfig(
            appId: "swift-life", appName: "Swift 生命周期", hostURL: "ws://\(addr)",
            lifecycle: LifecyclePolicy(
                mode: .idle, idleTimeoutMs: 300, residency: .exitAlways,
                wake: .urlScheme("swiftlife", background: true)
            ),
            onIdleExit: { idleExit.fulfill() }
        ))
        try client.backgroundTool("echo", description: "回显") { (g: Greeting, ctx) in
            // handler 发起的后续工作：持有 200ms 后释放，期间不应休眠。
            let hold = try ctx.hold()
            DispatchQueue.global().asyncAfter(deadline: .now() + 0.2) { hold.release() }
            return g.name
        }
        try client.backgroundTool("stock.reserve", description: "预留库存") { (_: NoArguments, _) -> Int in
            throw ToolCallError(ErrorKind.invalidInput, "库存不足", details: .object(["available": .number(2)]))
        }
        client.start()

        let lines: [[String: Any]] = await Task.detached {
            var lines: [[String: Any]] = []
            while true {
                let line = self.readLine(handle)
                if line.isEmpty { break }
                guard let obj = try? JSONSerialization.jsonObject(with: Data(line.utf8)) as? [String: Any] else { continue }
                lines.append(obj)
                if obj["type"] as? String == "wake", let arg = obj["arg"] as? String {
                    XCTAssertTrue(client.handleWake(arg))
                }
            }
            return lines
        }.value
        host.waitUntilExit()
        await fulfillment(of: [idleExit], timeout: 5)
        client.stop()
        XCTAssertEqual(host.terminationStatus, 0, "\(lines)")
        for l in lines { print("fake_host:", String(decoding: try JSONSerialization.data(withJSONObject: l, options: [.sortedKeys]), as: UTF8.self)) }

        let types = lines.map { $0["type"] as? String ?? "?" }
        XCTAssertEqual(types, ["tools", "sleep", "wake", "hello", "tools", "invoke", "invoke", "sleep"])
        XCTAssertEqual(lines[1]["accepted"] as? Bool, true)
        XCTAssertEqual(lines[1]["reason"] as? String, "idle")
        let hello = lines[3]
        XCTAssertEqual(hello["toolsCurrent"] as? Bool, true)
        XCTAssertEqual(hello["wakeReason"] as? String, "os-activation")
        XCTAssertEqual(hello["launchToken"] as? String, lines[2]["token"] as? String)
        XCTAssertEqual(lines[4]["synced"] as? Bool, false)
        XCTAssertEqual((lines[5]["result"] as? [String: Any])?["data"] as? String, "醒了")
        let err = lines[6]["error"] as? [String: Any]
        let data = err?["data"] as? [String: Any]
        XCTAssertEqual(data?["kind"] as? String, "INVALID_INPUT")
        XCTAssertEqual(data?["available"] as? Int, 2)
        XCTAssertEqual(lines[7]["accepted"] as? Bool, true)
        XCTAssertEqual(lines[7]["toolsHash"] as? String, client.toolsHash)
    }
}
