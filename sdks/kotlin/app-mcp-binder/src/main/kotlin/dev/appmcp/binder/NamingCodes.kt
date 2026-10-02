package dev.appmcp.binder

/**
 * 按名寻址的错误码（spec/naming.md 第 12 节；与 Rust `app_mcp_protocol::naming::codes` 同一组字符串，Hub 据此还原）。
 * 出现在 [ChannelOpenException.code]、Binder 回复（`<CODE>：<说明>`）与 Hub 的拨号结果中。
 */
object NamingCodes {
    /** 目标未安装 / 组件不存在（未声明对应 Intent 动作的 Service）。 */
    const val NAME_NOT_FOUND = "NAME_NOT_FOUND"

    /** 激活失败的其他情况（对端在连接前断开、Binder 调用失败、对端启动失败等）。 */
    const val ACTIVATION_DENIED = "ACTIVATION_DENIED"

    /** 已发出绑定，但超时内没有连上。 */
    const val ACTIVATION_TIMEOUT = "ACTIVATION_TIMEOUT"

    /** 无权绑定：目标 Service 未导出，或要求本应用未获得的权限（开发配置问题，用户无法在设置中解决）。 */
    const val BIND_PERMISSION_DENIED = "BIND_PERMISSION_DENIED"

    /** 对端（App）拒绝了拨号的 Hub（spec/naming.md 10.2）。 */
    const val HUB_NOT_TRUSTED = "HUB_NOT_TRUSTED"

    /** 对端同时通道数已达上限。 */
    const val CHANNEL_LIMIT = "CHANNEL_LIMIT"

    /**
     * 目标已安装、组件存在，但系统拒绝绑定（`bindService` 返回 false 或 `SecurityException`：关联启动 / 自启动管控、
     * OEM 拦截，如 Flyme `requires a ifw permit`）；需用户在系统设置中放行（[BlockedTarget]）。
     */
    const val ACTIVATION_BLOCKED = "ACTIVATION_BLOCKED"

    /** 独立 Hub App 的原生库未包含 MCP 出口（cargo feature `mcp-server`），无法为 Agent 提供 MCP。 */
    const val HUB_UNSUPPORTED = "HUB_UNSUPPORTED"
}
