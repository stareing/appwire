// verify.sh 的 deprecated 步骤：实现 deprecated.json 生成的 LegacyToolHandlers（实现已弃用的要求不产生警告）并检查 JSON 往返。
import Foundation

struct Impl: LegacyToolHandlers {
    func ordersList(_ params: OrdersListParams) async throws -> any Encodable & Sendable { params }
    func ordersFind(_ params: OrdersFindParams) async throws -> any Encodable & Sendable { params }
    func cartLegacyClear(_ params: CartLegacyClearParams) async throws -> any Encodable & Sendable { "cleared" }
}

func check(_ ok: Bool, _ what: String) {
    if !ok { fatalError("检查失败：\(what)") }
}

func encoded(_ value: (any Encodable & Sendable)?) throws -> String {
    guard let value else { return "nil" }
    let encoder = JSONEncoder()
    encoder.outputFormatting = .sortedKeys
    return String(decoding: try encoder.encode(value), as: UTF8.self)
}

let list = try await LegacyTools.dispatch(Impl(), name: "orders.list", arguments: Data(#"{"status":"paid","page":2}"#.utf8))
check(try encoded(list) == #"{"page":2,"status":"paid"}"#, "弃用属性往返：\(try encoded(list))")
let find = try await LegacyTools.dispatch(Impl(), name: "orders.find", arguments: Data(#"{"state":"s","filter":{"legacyTag":"t"}}"#.utf8))
check(try encoded(find) == #"{"filter":{"legacyTag":"t"},"state":"s"}"#, "嵌套弃用属性：\(try encoded(find))")
let clear = try await LegacyTools.dispatch(Impl(), name: "cart.legacyClear", arguments: Data()) as? String
check(clear == "cleared", "弃用工具分派")
print("ok")
