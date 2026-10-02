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

/// 嵌入式 Hub（随机端口）+ 同进程 App 端 SDK（AppMcpClient）经真实 WebSocket / 本地 IPC 连上。
/// 测试关闭默认 IPC 端点（`enableIpc: false`），不占用本机常驻 Host 的端点；IPC 用临时端点单独测试。
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
        let hub = try Hub(config: HubConfig(listen: "127.0.0.1:0", enableIpc: false, approvalMinRisk: .destructive))
        defer { hub.close() }
        let approvals = Approvals()
        hub.setApprovalHandler { req in
            await approvals.record(req)
            return false
        }
        var events = hub.events().makeAsyncIterator()

        let app = try AppMcpClient(config: AppMcpConfig(
            appId: "notes", appName: "笔记", hostURL: "ws://\(hub.listenAddr ?? "")/app"
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

        // 运行状态：实例的连接 ID 与 App 端 SDK 看到的一致
        let st = try hub.status()
        XCTAssertEqual(st.service, "app-mcp")
        XCTAssertEqual(st.listen, hub.listenAddr)
        XCTAssertTrue(st.reports.isEmpty)
        let notes = try XCTUnwrap(st.apps.first { $0.appId == "notes" })
        XCTAssertEqual(notes.state, .connected)
        XCTAssertEqual(notes.instances.first?.state, .connected)
        let cid = notes.instances.first?.info.connectionId
        XCTAssertNotNil(cid)
        XCTAssertEqual(cid, app.connectionId)
        XCTAssertEqual(hub.apps().first { $0.appId == "notes" }?.instances.first?.connectionId, cid)

        // callTool（Encodable 参数）
        let r = try await hub.callTool("notes.add", arguments: NoteArgs(text: "买牛奶"), timeout: 5)
        XCTAssertNil(r.error)
        XCTAssertEqual(try r.decode(Saved.self), Saved(saved: "买牛奶"))
        // Hub API 调用方的 Agent 任务（spec/hub-api.md 3.6）
        let task: AgentTaskStatus = try XCTUnwrap(try hub.status().tasks?.first { $0.caller == "api" })
        XCTAssertEqual(task.kind, CallerKind.api)
        XCTAssertTrue(task.id.hasPrefix("task-"), task.id)

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
        // principal / clientName 只在 MCP 出口发起的审批中出现
        XCTAssertNil(seen.first?.principal ?? nil)
        XCTAssertNil(seen.first?.clientName ?? nil)

        // @MainActor handler（UI 确认）→ 同意 → 成功；抛出错误 → 拒绝
        hub.setApprovalHandler { @MainActor _ in
            dispatchPrecondition(condition: .onQueue(.main))
            return true
        }
        let approved = try await hub.callTool("notes.clear", timeout: 5)
        XCTAssertNil(approved.error)
        hub.setApprovalHandler { _ in throw WakeFailed(message: "UI 崩溃") }
        let failed = try await hub.callTool("notes.clear", timeout: 5)
        XCTAssertEqual(failed.error?.kind, "USER_REJECTED")

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
            listen: "127.0.0.1:0", enableIpc: false, listChangedDebounceMs: 20, leaseTtlMs: 0, wakeTimeoutMs: 10_000
        ))
        defer { hub.close() }
        var events = hub.events().makeAsyncIterator()
        let app = try AppMcpClient(config: AppMcpConfig(
            appId: "sleepy", appName: "会睡觉的 App", hostURL: "ws://\(hub.listenAddr ?? "")/app", instanceId: "s1",
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

    /// 第 14 / 19 项：限流 / 大小上限配置与统计、工具注解 / outputSchema、结构化调用结果。
    func testLimitsAnnotationsAndStructuredResult() async throws {
        // 非法：限流时 burst 须 ≥ 1
        XCTAssertThrowsError(try Hub(config: HubConfig(
            listen: "127.0.0.1:0", enableIpc: false, limits: LimitsConfig(toolRateBurst: 0)
        ))) { error in
            guard case HubError.InvalidConfig = error else { return XCTFail("\(error)") }
        }
        let hub = try Hub(config: HubConfig(
            listen: "127.0.0.1:0", enableIpc: false,
            limits: LimitsConfig(toolRatePerMinute: 1, toolRateBurst: 1, maxArgumentsBytes: 64),
            outputValidation: .reject
        ))
        defer { hub.close() }
        let app = try AppMcpClient(config: AppMcpConfig(
            appId: "orders", appName: "订单", hostURL: "ws://\(hub.listenAddr ?? "")/app"
        ))
        let schema = #"{"type":"object","properties":{"orderId":{"type":"string"}}}"#
        try app.tool(
            "submit", description: "下单",
            annotations: ToolAnnotations(idempotentHint: false), outputSchema: schema
        ) { (_: NoArguments, _) in
            ToolResult(
                data: ["orderId": "o1"], status: .pending, stateResource: "order.state",
                summary: "已提交，等待付款", annotations: ContentAnnotations(priority: 0.5)
            )
        }
        try app.tool("echo", description: "回显") { (args: JSONValue, _) in args }
        app.start()
        defer { app.stop() }

        let deadline = Date().addingTimeInterval(10)
        var tools: [HubTool] = []
        while tools.count < 2 || tools.contains(where: { $0.availability != .available }) {
            if Date() > deadline { return XCTFail("等待工具注册超时") }
            try await Task.sleep(nanoseconds: 20_000_000)
            tools = hub.tools(ToolFilter(apps: ["orders"], includeBuiltin: false))
        }
        let submit = try XCTUnwrap(tools.first { $0.tool == "submit" })
        XCTAssertEqual(submit.annotations.idempotentHint, false)
        XCTAssertEqual(submit.annotations.readOnlyHint, false, "缺少的字段按 risk（write）推导")
        let declared = try JSONSerialization.jsonObject(with: Data(try XCTUnwrap(submit.outputSchemaJson).utf8)) as? NSDictionary
        XCTAssertEqual(declared, try JSONSerialization.jsonObject(with: Data(schema.utf8)) as? NSDictionary)
        XCTAssertNil(tools.first { $0.tool == "echo" }?.outputSchemaJson)

        let out = try await hub.callTool("orders.submit")
        XCTAssertNil(out.error)
        XCTAssertEqual(out.status, .pending)
        XCTAssertEqual(out.stateResource, "app-mcp://orders/order.state")
        XCTAssertEqual(out.summary, "已提交，等待付款")
        XCTAssertEqual(out.annotations?.priority, 0.5)
        let limited = try await hub.callTool("orders.submit")
        XCTAssertEqual(limited.error?.kind, "RATE_LIMITED")
        let big = try await hub.callTool("orders.echo", argumentsJSON: #"{"text":"\#(String(repeating: "x", count: 100))"}"#)
        XCTAssertEqual(big.error?.kind, "PAYLOAD_TOO_LARGE")
        let plain = try await hub.callTool("orders.echo", argumentsJSON: #"{"a":1}"#)
        XCTAssertNil(plain.error)
        XCTAssertEqual(plain.status, .done)

        let st = try hub.status()
        XCTAssertEqual(st.limits?.toolRatePerMinute, 1)
        XCTAssertEqual(st.limits?.maxArgumentsBytes, 64)
        XCTAssertEqual(st.limits?.appRatePerMinute, 600)
        XCTAssertEqual(st.outputValidation, .reject)
        let orders = try XCTUnwrap(st.apps.first { $0.appId == "orders" })
        XCTAssertEqual(orders.rateLimited, 1)
        XCTAssertEqual(orders.tooLarge, 1)
        let decl = try XCTUnwrap(orders.tools.first { $0.name == "submit" })
        XCTAssertEqual(decl.risk, .write)
        XCTAssertEqual(decl.annotations?.idempotentHint, false)
        XCTAssertEqual(decl.effective.readOnlyHint, false)
        XCTAssertTrue(decl.outputSchema)
    }

    /// spec/hub-api.md 3.14 / 3.15：`HubTool.surface` / `page`、`callTool(idempotencyKey:)` 原样转交、`routedTo`、`navigateTimeoutMs`。
    func testSurfacePageAndIdempotencyKey() async throws {
        let hub = try Hub(config: HubConfig(listen: "127.0.0.1:0", enableIpc: false, navigateTimeoutMs: 800))
        defer { hub.close() }
        let app = try AppMcpClient(config: AppMcpConfig(
            appId: "cafe", appName: "咖啡", hostURL: "ws://\(hub.listenAddr ?? "")/app"
        ))
        try app.tool("cart.checkout", description: "结算", surface: .view, page: "cart") { (_: NoArguments, ctx) in
            ["key": ctx.idempotencyKey ?? ""]
        }
        try app.tool("order.submit", description: "下单") { (_: NoArguments, ctx) in ["key": ctx.idempotencyKey ?? ""] }
        app.start()
        defer { app.stop() }

        let deadline = Date().addingTimeInterval(10)
        var tools: [HubTool] = []
        while tools.count < 2 || tools.contains(where: { $0.availability != .available }) {
            if Date() > deadline { return XCTFail("等待工具注册超时") }
            try await Task.sleep(nanoseconds: 20_000_000)
            tools = hub.tools(ToolFilter(apps: ["cafe"], includeBuiltin: false))
        }
        let checkout = try XCTUnwrap(tools.first { $0.tool == "cart.checkout" })
        XCTAssertEqual(checkout.surface, HubToolSurface.view)
        XCTAssertEqual(checkout.page, "cart")
        let submit = try XCTUnwrap(tools.first { $0.tool == "order.submit" })
        XCTAssertEqual(submit.surface, HubToolSurface.app)
        XCTAssertNil(submit.page)
        let builtins = hub.tools().filter { $0.name.hasPrefix("apps.") }
        for n in ["apps.activate", "apps.release", "apps.page", "apps.navigate"] {
            XCTAssertTrue(builtins.contains { $0.name == n }, "缺少内置工具 \(n)")
        }
        XCTAssertTrue(builtins.allSatisfy { $0.surface == nil && $0.page == nil })

        let out = try await hub.callTool("cafe.order.submit", idempotencyKey: "order-7")
        XCTAssertNil(out.error)
        XCTAssertEqual(out.dataJSON, #"{"key":"order-7"}"#)
        XCTAssertNil(out.routedTo)
        XCTAssertFalse(out.woke, "已连接的 App 不唤醒")
        let bad = try await hub.callTool("cafe.order.submit", idempotencyKey: "")
        XCTAssertEqual(bad.error?.kind, "INVALID_INPUT")
    }

    /// 第 16 项 O2：`callTool(onProgress:)` 在返回前收到合并后的进度；资源内容标注经 Hub 列出。
    func testProgressCallbackAndResourceAnnotations() async throws {
        let hub = try Hub(config: HubConfig(listen: "127.0.0.1:0", enableIpc: false))
        defer { hub.close() }
        let app = try AppMcpClient(config: AppMcpConfig(
            appId: "work", appName: "长任务", hostURL: "ws://\(hub.listenAddr ?? "")/app"
        ))
        try app.backgroundTool("run", description: "长任务", risk: .read) { (_: NoArguments, ctx) in
            ctx.progress(1, total: 2, message: "第一步")
            try await Task.sleep(nanoseconds: 350_000_000) // 超过 Hub 默认合并间隔 250 ms
            ctx.progress(2, total: 2)
            try await Task.sleep(nanoseconds: 350_000_000)
            return ["done": true]
        }
        try app.resource(
            "cart", description: "购物车",
            annotations: AppMcp.ContentAnnotations(audience: [.user], priority: 0.5)
        ) { [String: String]() }
        app.start()
        defer { app.stop() }

        let deadline = Date().addingTimeInterval(10)
        while hub.tools(ToolFilter(apps: ["work"], includeBuiltin: false)).isEmpty
            || !hub.resources().contains(where: { $0.appId == "work" }) {
            if Date() > deadline { return XCTFail("等待注册超时") }
            try await Task.sleep(nanoseconds: 20_000_000)
        }
        let res = try XCTUnwrap(hub.resources().first { $0.appId == "work" })
        XCTAssertEqual(res.annotations?.priority, 0.5)
        XCTAssertEqual(res.annotations?.audience, [.user])

        let got = ProgressLog()
        let out = try await hub.callTool("work.run", onProgress: { got.append($0) })
        XCTAssertNil(out.error)
        XCTAssertEqual(
            got.items,
            [ProgressUpdate(progress: 1, total: 2, message: "第一步"), ProgressUpdate(progress: 2, total: 2, message: nil)],
            "结果返回前收到全部进度"
        )
    }

    /// 策略挂点（spec/hub-api.md 3.13）：hide → 不在列表、调用 TOOL_NOT_FOUND；deny → POLICY_DENIED；
    /// setPolicy 不合法时抛错且旧规则继续生效；清空后恢复原行为。
    func testPolicyHideDenyAndSetPolicy() async throws {
        XCTAssertThrowsError(try Hub(config: HubConfig(
            listen: "127.0.0.1:0", enableIpc: false,
            policy: PolicyConfig(rules: [PolicyRule(id: "bad id", action: .hide, app: "notes")])
        ))) { error in
            guard case HubError.InvalidConfig = error else { return XCTFail("\(error)") }
        }
        let hub = try Hub(config: HubConfig(
            listen: "127.0.0.1:0", enableIpc: false,
            policy: PolicyConfig(rules: [
                PolicyRule(id: "hide-clear", action: .hide, app: "notes", tool: "clear"),
                PolicyRule(id: "deny-add", action: .deny, app: "notes", tool: "add"),
            ])
        ))
        defer { hub.close() }
        let app = try AppMcpClient(config: AppMcpConfig(
            appId: "notes", appName: "笔记", hostURL: "ws://\(hub.listenAddr ?? "")/app"
        ))
        try app.tool("add", description: "添加") { (args: JSONValue, _) in args }
        try app.tool("clear", description: "清空") { (args: JSONValue, _) in args }
        try app.tool("echo", description: "回显") { (args: JSONValue, _) in args }
        app.start()
        defer { app.stop() }

        func names() -> [String] {
            hub.tools(ToolFilter(apps: ["notes"], includeBuiltin: false)).map(\.tool).sorted()
        }
        let deadline = Date().addingTimeInterval(10)
        while names().count < 2 {
            if Date() > deadline { return XCTFail("等待工具注册超时") }
            try await Task.sleep(nanoseconds: 20_000_000)
        }
        XCTAssertEqual(names(), ["add", "echo"], "hide 的工具不在列表中")

        let hidden = try await hub.callTool("notes.clear")
        XCTAssertEqual(hidden.error?.kind, "TOOL_NOT_FOUND")
        let denied = try await hub.callTool("notes.add")
        XCTAssertEqual(denied.error?.kind, "POLICY_DENIED")
        let details = try JSONSerialization.jsonObject(
            with: Data(try XCTUnwrap(denied.error?.detailsJson).utf8)
        ) as? [String: Any]
        XCTAssertEqual(details?["ruleId"] as? String, "deny-add")
        XCTAssertEqual(details?["hook"] as? String, "call")
        let st = try hub.policy()
        XCTAssertEqual(st.rules.map(\.rule.id), ["hide-clear", "deny-add"])
        XCTAssertEqual(st.rules.map(\.hits), [1, 1])
        XCTAssertEqual(try hub.status().policy?.rules.count, 2)

        // 不合法：hide 不能写 hooks → 抛错，旧规则继续生效
        XCTAssertThrowsError(try hub.setPolicy(PolicyConfig(rules: [
            PolicyRule(id: "x", action: .hide, app: "notes", hooks: [.call]),
        ]))) { error in
            guard case let HubError.Tool(kind, _, _) = error else { return XCTFail("\(error)") }
            XCTAssertEqual(kind, "INVALID_INPUT")
        }
        let still = try await hub.callTool("notes.add")
        XCTAssertEqual(still.error?.kind, "POLICY_DENIED")
        XCTAssertNotNil(try hub.policy().lastError)

        // 清空：恢复原行为
        try hub.setPolicy(PolicyConfig(rules: []))
        XCTAssertEqual(names(), ["add", "clear", "echo"])
        let ok = try await hub.callTool("notes.add")
        XCTAssertNil(ok.error)
        XCTAssertNil(try hub.policy().lastError)
    }

    /// spec/hub-api.md 3.6 / 3.7：无会话 MCP 请求的配置可设置；启动时没有 Agent 任务。
    func testStatelessConfig() throws {
        let config = HubConfig(
            enableListen: false, enableIpc: false, taskIdleTtlMs: 0, statelessToolExposure: .progressive,
            principalSelectTtlMs: 1500, statelessListTtlMs: 750
        )
        XCTAssertEqual(config.statelessToolExposure, .progressive)
        let hub = try Hub(config: config)
        XCTAssertEqual(try hub.status().tasks, [])
        hub.close()
    }

    /// spec/hub-api.md 3.6：MCP 出口协议版本与 listen 上限可设置；status 报告 listen 流数。
    func testMcpListenConfig() throws {
        let config = HubConfig(
            enableListen: false, enableIpc: false, mcpProtocolMode: .legacyOnly, maxListenStreams: 0, maxListenResources: 8
        )
        XCTAssertEqual(config.mcpProtocolMode, .legacyOnly)
        let hub = try Hub(config: config)
        XCTAssertEqual(try hub.status().mcpListenStreams, 0)
        hub.close()
    }

    /// spec/hub-api.md 3.6「任务句柄」：每主体任务句柄上限可设置（0 关闭），缺省为空即取 Hub 默认。
    func testMaxTaskHandlesConfig() throws {
        XCTAssertNil(HubConfig().maxTaskHandles)
        for n: UInt32 in [0, 5] {
            let config = HubConfig(enableListen: false, enableIpc: false, maxTaskHandles: n)
            XCTAssertEqual(config.maxTaskHandles, n)
            let hub = try Hub(config: config)
            XCTAssertEqual(try hub.status().mcpSessions, 0)
            hub.close()
        }
    }

    func testProgressiveExposureConfig() throws {
        let hub = try Hub(config: HubConfig(
            enableListen: false, enableIpc: false, waker: .exec(argv: ["true"]), toolExposure: .progressive, toolExposureThreshold: 5
        ))
        // 渐进暴露：没有展开的 App 时只有内置工具（含 apps.tools）
        XCTAssertEqual(hub.tools(ToolFilter(session: "c1")).map(\.name),
                       ["apps.list", "apps.select", "apps.overview", "apps.tools", "apps.activate", "apps.release"])
        hub.close()
    }

    /// 关闭的能力报 `.Unsupported`（与 `.Io` 区分）。本机库为完整能力，调用成功；精简构建由 Android HubSelfTest 覆盖。
    func testUnsupportedFeatureIsDistinctCategory() async throws {
        func check(_ body: () async throws -> Void) async {
            do { try await body() } catch {
                guard case .Unsupported? = error as? HubError else { return XCTFail("应为 Unsupported：\(error)") }
            }
        }
        await check { try Hub(config: HubConfig(listen: "127.0.0.1:0", mcpHttp: true, enableIpc: false)).close() }
        let hub = try Hub(config: HubConfig(listen: "127.0.0.1:0", enableIpc: false))
        await check { _ = try await hub.serveHTTP("127.0.0.1:0") }
        hub.close()
        let e = HubError.Unsupported(detail: "缺少 `mcp-server`")
        XCTAssertNotEqual(e, .Io(detail: "缺少 `mcp-server`"))
    }

    func testFormatsAndShutdown() async throws {
        XCTAssertEqual(try ToolFormat.parse("anthropic"), .anthropic)
        XCTAssertThrowsError(try ToolFormat.parse("nope"))
        let hub = try Hub(config: HubConfig(enableListen: false, enableIpc: false))
        XCTAssertNil(hub.listenAddr)
        XCTAssertNil(hub.ipcEndpoint)
        XCTAssertEqual(
            Set(hub.tools().map(\.name)),
            ["apps.list", "apps.select", "apps.overview", "apps.activate", "apps.release"]
        )
        let st = try hub.status()
        XCTAssertNil(st.listen)
        XCTAssertTrue(st.apps.isEmpty && !st.mcpHttp && !st.auth.tokenConfigured)
        hub.close()
        hub.close() // 幂等
        XCTAssertThrowsError(try hub.status()) { XCTAssertEqual($0 as? HubError, .Shutdown) }
        do {
            _ = try await hub.callTool("apps.list")
            XCTFail("关闭后应抛出 HubError")
        } catch is HubError {}
    }

    func testNativeAppOverIpc() async throws {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("app-mcp-swift-ipc-\(ProcessInfo.processInfo.processIdentifier)")
        defer { try? FileManager.default.removeItem(at: dir) }
        let endpoint = "unix:\(dir.appendingPathComponent("run/hub.sock").path)"
        let hub = try Hub(config: HubConfig(enableListen: false, ipcEndpoint: endpoint))
        defer { hub.close() }
        XCTAssertEqual(hub.ipcEndpoint, endpoint)
        let app = try AppMcpClient(config: AppMcpConfig(appId: "notes", appName: "笔记", hostURL: endpoint))
        let schema = #"{"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}"#
        try app.tool("add", description: "添加笔记", inputSchema: schema, risk: .write) { (args: NoteArgs, _) in
            Saved(saved: args.text)
        }
        app.start()
        defer { app.stop() }
        try await waitTools(hub, 1)
        let instance = hub.apps().first { $0.appId == "notes" }?.instances.first
        XCTAssertEqual(instance?.pid, UInt32(ProcessInfo.processInfo.processIdentifier))
    }
}

/// 进度回调记录（回调在 Hub 线程上）。
private final class ProgressLog: @unchecked Sendable {
    private let lock = NSLock()
    private var list: [ProgressUpdate] = []
    func append(_ u: ProgressUpdate) { lock.lock(); list.append(u); lock.unlock() }
    var items: [ProgressUpdate] { lock.lock(); defer { lock.unlock() }; return list }
}
