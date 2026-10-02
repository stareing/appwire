@testable import AppMcp
import AppMcpBindings
import XCTest

/// 导航表、工具补丁更新与 surface / page（spec/protocol.md 3.4）。
/// @compat 本机没有 Swift 工具链，未运行过。
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
            outputSchemaJson: #"{"type":"object"}"#, surface: .view, page: "cart"
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
        decl.surface = .app
        XCTAssertNil(decl.applied(to: base).surface, "app 为缺省，不序列化")
    }
}
