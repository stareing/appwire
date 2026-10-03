@testable import AppMcp
import AppMcpBindings
import XCTest

/// 导航表、工具补丁更新与 surface / page（spec/protocol.md 3.4）。
final class NavigationTests: XCTestCase {
    @MainActor
    func testPageRouterDispatchesByName() async throws {
        var seen: [String] = []
        let router = PageRouter()
            .page("cart") { req in seen.append("cart:\(req.paramsJSON ?? "")") }
            .pageWithResult("login") { _ in .denied("需要先登录") }
        let ok = try await router.navigate(NavigationRequest(page: "cart", paramsJSON: #"{"id":7}"#))
        XCTAssertEqual(ok, .ok)
        XCTAssertEqual(seen, [#"cart:{"id":7}"#])
        let denied = try await router.navigate(NavigationRequest(page: "login", paramsJSON: nil))
        XCTAssertEqual(denied, .denied("需要先登录"))
        let unknown = try await router.navigate(NavigationRequest(page: "nowhere", paramsJSON: nil))
        XCTAssertEqual(unknown, .failed("未知页面：nowhere"))
        XCTAssertEqual(router.pageNames, ["cart", "login"])
    }

    func testNavigationRequestDecodesParams() throws {
        struct P: Decodable, Equatable { let id: String }
        XCTAssertEqual(try NavigationRequest(page: "o", paramsJSON: #"{"id":"o1"}"#).params(P.self), P(id: "o1"))
        XCTAssertNil(try NavigationRequest(page: "o", paramsJSON: nil).params(P.self))
    }

    func testToolDeclarationClearsOnlyChangedFields() {
        let base = ToolSpec(
            name: "t", description: "旧", title: "标题",
            annotations: ToolAnnotations(title: nil, readOnlyHint: true, destructiveHint: nil, idempotentHint: nil, openWorldHint: nil),
            outputSchemaJson: #"{"type":"object"}"#, surface: .view, page: "cart", backgroundTool: "t.bg"
        )
        var decl = ToolDeclaration(base)
        decl.description = "新"
        decl.annotations = nil
        decl.outputSchema = nil
        let next = decl.applied(to: base)
        XCTAssertEqual(next.description, "新")
        XCTAssertNil(next.annotations)
        XCTAssertNil(next.outputSchemaJson)
        XCTAssertEqual(next.title, "标题")
        XCTAssertEqual(next.page, "cart")
        XCTAssertEqual(next.surface, .view)
        XCTAssertEqual(next.backgroundTool, "t.bg")
        decl.backgroundTool = nil
        XCTAssertNil(decl.applied(to: base).backgroundTool)
        decl.surface = .app
        XCTAssertNil(decl.applied(to: base).surface, "app 为缺省，不序列化")
    }

    func testToolDeclarationKeepsCallScheduling() {
        let base = ToolSpec(name: "t", description: "旧", concurrency: 2, exclusive: "doc")
        var decl = ToolDeclaration(base)
        XCTAssertEqual(decl.concurrency, 2)
        XCTAssertEqual(decl.exclusive, "doc")
        decl.description = "新"
        let kept = decl.applied(to: base)
        XCTAssertEqual(kept.concurrency, 2, "补丁型 update 不得重置调度声明")
        XCTAssertEqual(kept.exclusive, "doc")
        decl.concurrency = 0
        decl.exclusive = nil
        let cleared = decl.applied(to: base)
        XCTAssertEqual(cleared.concurrency, 0)
        XCTAssertNil(cleared.exclusive)
    }

    func testThrownErrorsMapToNavigationResults() throws {
        let ua = ToolCallError.userActionRequired(message: "请点开通知", reason: UserActionReason.foreground, uri: "shop://cart")
        XCTAssertEqual(
            navigationFailure(kind: ua.kind, message: ua.message, detailsJSON: try ua.details.map(encodeJSON)),
            .userActionRequired(message: "请点开通知", reason: "foreground", uri: "shop://cart")
        )
        XCTAssertEqual(
            navigationFailure(kind: ErrorKind.userActionRequired, message: "只有说明", detailsJSON: nil),
            .userActionRequired(message: "只有说明")
        )
        XCTAssertEqual(navigationFailure(kind: ErrorKind.navigationDenied, message: "不行", detailsJSON: nil), .denied("不行"))
        XCTAssertEqual(navigationFailure(kind: ErrorKind.handlerError, message: "坏了", detailsJSON: nil), .failed("坏了"))
    }

    func testNavigateInBackgroundConfigAndSetter() throws {
        let client = try AppMcpClient(config: AppMcpConfig(
            appId: "swift-nav-bg", appName: "后台导航", hostURL: "ws://127.0.0.1:9", navigateInBackground: true
        ))
        client.setNavigateInBackground(false)
        client.stop()
    }
}
