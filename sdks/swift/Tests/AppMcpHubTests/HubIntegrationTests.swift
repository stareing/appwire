import AppMcp
import AppMcpHub
import Foundation
import XCTest

private struct NoteArgs: Codable, Sendable {
    let text: String
}

private struct Saved: Codable, Sendable, Equatable {
    let saved: String
}

/// 记录审批请求并一律拒绝。
private actor Approvals {
    private(set) var requests: [ApprovalRequest] = []
    func record(_ r: ApprovalRequest) { requests.append(r) }
}

private actor Wakes {
    private(set) var requests: [WakeRequest] = []
    func record(_ r: WakeRequest) { requests.append(r) }
}

/// 嵌入式 Hub（随机端口）+ 同进程 App 端 SDK（AppMcpClient）经真实 WebSocket 连上。
final class HubIntegrationTests: XCTestCase {
    private func next(
        _ it: inout AsyncStream<HubEvent>.Iterator,
        timeout: TimeInterval = 10,
        _ pred: (HubEvent) -> Bool
    ) async throws -> HubEvent {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            guard let e = await it.next() else { break }
            if pred(e) { return e }
        }
        throw XCTSkip("等待事件超时")
    }

    private func waitTools(_ hub: Hub, _ n: Int) async throws {
        let deadline = Date().addingTimeInterval(10)
        while hub.tools(ToolFilter(apps: ["notes"], includeBuiltin: false)).count < n {
            if Date() > deadline { XCTFail("等待工具注册超时"); return }
            try await Task.sleep(nanoseconds: 20_000_000)
        }
    }

    func testEndToEnd() async throws {
        let hub = try Hub(config: HubConfig(wsAddr: "127.0.0.1:0", approvalMinRisk: .destructive))
        defer { hub.close() }
        let approvals = Approvals()
        hub.setApprovalHandler { req in
            await approvals.record(req)
            return false
        }
        var events = hub.events().makeAsyncIterator()

        let app = try AppMcpClient(config: AppMcpConfig(
            appId: "notes", appName: "笔记", hostURL: "ws://\(hub.wsAddr ?? "")"
        ))
        let schema = #"{"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}"#
        try app.tool("add", description: "添加笔记", inputSchema: schema, risk: .write) { (args: NoteArgs, _) in
            Saved(saved: args.text)
        }
        try app.tool("clear", description: "清空笔记", risk: .destructive) { (_: NoArguments, _) in
            ["cleared": true]
        }
        app.start()
        defer { app.stop() }

        // 事件：App 连上
        let connected = try await next(&events) {
            if case let .appConnected(appId, _) = $0 { return appId == "notes" }
            return false
        }
        print("收到事件：\(connected)")

        // 列工具
        try await waitTools(hub, 2)
        let tools = hub.tools(ToolFilter(apps: ["notes"], includeBuiltin: false))
        XCTAssertEqual(Set(tools.map(\.name)), ["notes.add", "notes.clear"])
        XCTAssertEqual(tools.first { $0.name == "notes.add" }?.availability, .available)
        XCTAssertTrue(hub.apps().contains { $0.appId == "notes" && $0.connected })

        // callTool（Encodable 参数）
        let r = try await hub.callTool("notes.add", arguments: NoteArgs(text: "买牛奶"), timeout: 5)
        XCTAssertNil(r.error)
        XCTAssertEqual(try r.decode(Saved.self), Saved(saved: "买牛奶"))

        // exportTools + dispatch（OpenAI Responses）
        let exported = hub.exportTools(.openAiResponses, filter: ToolFilter(apps: ["notes"]))
        let defs = try JSONSerialization.jsonObject(with: Data(exported.utf8)) as? [[String: Any]] ?? []
        XCTAssertTrue(defs.contains { $0["name"] as? String == "notes__add" }, exported)
        let call = #"{"type":"function_call","call_id":"fc_1","name":"notes__add","arguments":"{\"text\":\"来自 LLM\"}"}"#
        let replyText = try await hub.dispatch(.openAiResponses, toolCall: call)
        let reply = try JSONSerialization.jsonObject(with: Data(replyText.utf8)) as? [String: Any] ?? [:]
        XCTAssertEqual(reply["type"] as? String, "function_call_output")
        XCTAssertEqual(reply["call_id"] as? String, "fc_1")
        XCTAssertTrue((reply["output"] as? String ?? "").contains("来自 LLM"), replyText)

        // 审批拒绝 → USER_REJECTED
        let rejected = try await hub.callTool("notes.clear", timeout: 5)
        XCTAssertEqual(rejected.error?.kind, "USER_REJECTED")
        XCTAssertThrowsError(try rejected.get()) { err in
            XCTAssertEqual((err as? ToolError)?.kind, "USER_REJECTED")
        }
        let seen = await approvals.requests
        XCTAssertEqual(seen.count, 1)
        XCTAssertEqual(seen.first?.appId, "notes")
        XCTAssertEqual(seen.first?.risk, .destructive)

        // 名称无法解析 → HubError
        do {
            _ = try await hub.callTool("nosuchapp.x")
            XCTFail("应抛出 HubError")
        } catch is HubError {}

        // 事件：App 断开
        app.stop()
        _ = try await next(&events) {
            if case let .appDisconnected(appId, _) = $0 { return appId == "notes" }
            return false
        }
    }

    func testDormantAppWokenByCustomWaker() async throws {
        let hub = try Hub(config: HubConfig(
            wsAddr: "127.0.0.1:0", listChangedDebounceMs: 20, leaseTtlMs: 0, wakeTimeoutMs: 10_000
        ))
        defer { hub.close() }
        var events = hub.events().makeAsyncIterator()
        let app = try AppMcpClient(config: AppMcpConfig(
            appId: "sleepy", appName: "会睡觉的 App", hostURL: "ws://\(hub.wsAddr ?? "")", instanceId: "s1",
            lifecycle: LifecyclePolicy(
                mode: .idle, idleTimeoutMs: 300,
                wake: AppMcp.WakeDescriptor(kind: .androidIntent, target: "dev.example/.WakeReceiver", background: true)
            )
        ))
        try app.tool("ping", description: "回显") { (_: NoArguments, _) in ["pong": true] }
        let wakes = Wakes()
        // 厂商在这里发送广播 / 打开 URL；测试中直接让同进程 App 处理激活参数。
        hub.setWaker { req in
            await wakes.record(req)
            if !app.handleWake(req.activationArg) { throw WakeFailed(message: "不认识的激活参数") }
        }
        app.start()
        defer { app.stop() }

        let dormant = try await next(&events) {
            if case let .appDormant(appId, _) = $0 { return appId == "sleepy" }
            return false
        }
        XCTAssertEqual(dormant, .appDormant(appId: "sleepy", instanceId: "s1"))
        let info = hub.apps().first { $0.appId == "sleepy" }
        XCTAssertEqual(info?.isDormant, true)
        XCTAssertEqual(info?.dormantInstances.map(\.instanceId), ["s1"])
        let tools = hub.tools(ToolFilter(apps: ["sleepy"], includeBuiltin: false))
        XCTAssertEqual(tools.map(\.availability), [.dormant])

        let r = try await hub.callTool("sleepy.ping", timeout: 10)
        XCTAssertNil(r.error, "\(r)")
        XCTAssertEqual(r.instanceId, "s1")
        let seen = await wakes.requests
        XCTAssertEqual(seen.count, 1)
        XCTAssertEqual(seen.first?.instanceId, "s1")
        XCTAssertEqual(seen.first?.descriptor.kind, HubWakeKind.androidIntent)
        XCTAssertEqual(seen.first?.activationArg, "app-mcp-wake:\(seen.first?.token ?? "")")

        // 失败类别透传；清除后恢复默认实现
        _ = try await next(&events) {
            if case .appDormant = $0 { return true }
            return false
        }
        hub.setWaker { _ in throw WakeFailed(kind: "APP_NOT_INSTALLED", message: "没装") }
        let bad = try await hub.callTool("sleepy.ping", timeout: 10)
        XCTAssertEqual(bad.error?.kind, "APP_NOT_INSTALLED")
        hub.setWaker(nil)
    }

    func testFormatsAndShutdown() async throws {
        XCTAssertEqual(try ToolFormat.parse("anthropic"), .anthropic)
        XCTAssertThrowsError(try ToolFormat.parse("nope"))
        let hub = try Hub(config: HubConfig(enableWs: false))
        XCTAssertNil(hub.wsAddr)
        XCTAssertEqual(Set(hub.tools().map(\.name)), ["apps.list", "apps.select", "apps.overview"])
        hub.close()
        hub.close() // 幂等
        do {
            _ = try await hub.callTool("apps.list")
            XCTFail("关闭后应抛出 HubError")
        } catch is HubError {}
    }
}
