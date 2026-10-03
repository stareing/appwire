import AppMcpBindings
import Foundation

// 事件（spec/protocol.md 3.5，第 16 项 N3）：声明可发出的事件、发出事件。Hub 投递到订阅方信箱，不代 Agent 发起调用。

extension AppMcpClient {
    /// 声明本实例可发出的事件（同名替换）；`payloadSchema` 为载荷的 JSON Schema 文本（描述用，Hub 不校验）。
    /// 已连接时随即同步给 Host，否则在下次握手成功后同步；不触发连接。
    ///
    /// @error 名称不合法 → `AppMcpError.InvalidName`；`payloadSchema` 不是合法 JSON → `.InvalidJson`；已停止 → `.Stopped`。
    public func declareEvent(_ name: String, description: String, payloadSchema: String? = nil) throws {
        try inner.declareEvent(info: EventInfo(name: name, description: description, payloadSchemaJson: payloadSchema))
    }

    /// 撤销事件声明；未声明过（或已停止）返回 `false`。
    @discardableResult
    public func removeEvent(_ name: String) -> Bool { inner.removeEvent(name: name) }

    /// 发出已声明的事件，`payloadJSON` 为 JSON 对象文本（`nil` = 无载荷）。
    ///
    /// 已连接时发送并返回 `true`；未连接（休眠、断线、重连中）丢弃并返回 `false`：不缓存、不为此连接或唤醒 Host，
    /// 也不推迟空闲休眠。需要可靠送达的状态变化请改用资源。
    ///
    /// @error 未声明、名称不合法 → `AppMcpError.InvalidName`；载荷不是合法 JSON、不是对象或超过 8 KiB → `.InvalidJson`；
    /// 已停止 → `.Stopped`。
    @discardableResult
    public func emitEvent(_ name: String, payloadJSON: String? = nil) throws -> Bool {
        try inner.emitEvent(name: name, payloadJson: payloadJSON)
    }

    /// 发出已声明的事件，载荷按 JSON 编码（须编码为对象，如 `struct` / `[String: …]`）。错误同 `emitEvent(_:payloadJSON:)`；
    /// 无法编码 → `AppMcpError.InvalidJson`。
    @discardableResult
    public func emitEvent<Payload: Encodable>(_ name: String, payload: Payload) throws -> Bool {
        let text: String
        do {
            text = String(decoding: try jsonEncoder.encode(payload), as: UTF8.self)
        } catch {
            throw AppMcpError.InvalidJson(detail: "事件载荷无法编码为 JSON：\(error)")
        }
        return try emitEvent(name, payloadJSON: text)
    }
}
