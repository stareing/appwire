//! 与头文件逐字段对应的 `#[repr(C)]` 配置与定义结构体。
//! 枚举字段用 c_int 接收，转换见 [`crate::convert`]。

use std::ffi::{c_char, c_int, c_void};

use app_mcp_native::LifecyclePolicy;

use crate::callbacks::{AmFreeFn, AmIdleExitFn, AmLogFn, AmPairedFn, AmStateFn};

// ---------------------------------------------------------------------------
// 配置与定义（与头文件逐字段对应；枚举字段用 c_int 接收，避免非法值造成未定义行为）
// ---------------------------------------------------------------------------

#[repr(C)]
pub struct AmClientConfig {
    pub app_id: *const c_char,
    pub app_name: *const c_char,
    pub instance_id: *const c_char,
    pub host_url: *const c_char,
    pub app_version: *const c_char,
    pub instance_title: *const c_char,
    pub token: *const c_char,
    pub launch_token: *const c_char,
    pub client_kind: c_int,
    pub max_concurrent_calls: u32,
    pub overview_summary: *const c_char,
    pub overview_body: *const c_char,
    pub overview_locale: *const c_char,
}

#[repr(C)]
pub struct AmClientCallbacks {
    pub on_state: Option<AmStateFn>,
    pub on_paired: Option<AmPairedFn>,
    pub on_log: Option<AmLogFn>,
    pub user_data: *mut c_void,
    pub free_user_data: Option<AmFreeFn>,
}

/// v3：生命周期策略（枚举字段用 c_int 接收）。
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct AmLifecycle {
    pub mode: c_int,
    pub idle_timeout_ms: u64,
    pub hidden_idle_timeout_ms: u64,
    pub grace_ms: u64,
    pub residency: c_int,
    pub wake_kind: c_int,
    pub wake_target: *const c_char,
    pub wake_background: bool,
}

/// v3：`am_client_new_ex` 的扩展选项（带 `struct_size`，按调用方给出的大小读取）。
#[repr(C)]
pub struct AmClientOptions {
    pub struct_size: u32,
    pub lifecycle: *const AmLifecycle,
    pub connect_timeout_ms: u32,
    pub on_idle_exit: Option<AmIdleExitFn>,
    /// v7（4e，spec/lifecycle.md 第 11 节）：`AmHeartbeatMode`，0 = AUTO。
    pub heartbeat: c_int,
    /// v7：连续多少次"Host 不在"后转休眠；0 = 默认（3），负数 = 一直重连。
    pub host_absent_retries: i32,
    /// v7：回退到 4e 之前的定时器行为。
    pub legacy_timers: bool,
    /// v8（4e 第二部分，spec/lifecycle.md 第 13 节 B1）：调用后的合并窗口；0 = 默认（2000），负数 = 不留窗口。
    pub merge_window_ms: i64,
    /// v8（B4）：进入后台且空闲时立即休眠。
    pub sleep_on_background: bool,
    /// v13（spec/protocol.md 3.3 调用去重）：首次结果的保留时长；0 = 默认（300000），负数 = 关闭去重。
    pub call_dedup_ttl_ms: i64,
    /// v13：最多保留的结果数；0 = 默认（64），负数 = 关闭去重。
    pub call_dedup_max_entries: i32,
    /// v17（spec/naming.md）：在系统名字服务登记（Linux：D-Bus `dev.appmcp.App.<appId>`），由 Hub 按名拨入。
    pub register_name: bool,
    /// v17：登记实例名（`[a-z][a-z0-9-]{0,31}`，不能是 `default`）；NULL = 只登记默认名字。
    pub name_instance: *const c_char,
}

/// v8：`am_resource_register_ex` 的资源选项（带 `struct_size`，按调用方给出的大小读取）。
#[repr(C)]
pub struct AmResourceOptions {
    pub struct_size: u32,
    /// 需实时推送（spec/lifecycle.md 第 13 节 B3）。
    pub realtime: bool,
    /// v13：资源内容的标注（MCP 内容注解 JSON 对象）；NULL = 未声明。
    pub annotations_json: *const c_char,
}

#[repr(C)]
pub struct AmToolSpec {
    pub name: *const c_char,
    pub description: *const c_char,
    pub input_schema_json: *const c_char,
    pub risk: c_int,
    pub activation: c_int,
    pub title: *const c_char,
    pub enabled: bool,
}

/// v9：`am_tool_register_ex` / `am_tool_update_ex` 的工具选项（带 `struct_size`，按调用方给出的大小读取）。
#[repr(C)]
pub struct AmToolOptions {
    pub struct_size: u32,
    /// 标准 MCP 工具注解（JSON 对象文本）；NULL = 未声明。
    pub annotations_json: *const c_char,
    /// 结果的 JSON Schema 文本（MCP `outputSchema`）；NULL = 未声明。
    pub output_schema_json: *const c_char,
    /// v14：所在页面名；NULL = 未声明。
    pub page: *const c_char,
    /// v14：`AmToolSurface`（0 = APP，1 = VIEW）。
    pub surface: c_int,
    /// v15：App 在后台时代替本工具（view 工具）调用的同 App app 工具本地名；NULL = 未声明。
    pub background_tool: *const c_char,
}

/// v9：`am_call_complete_ex` 的调用结果（带 `struct_size`，按调用方给出的大小读取；`status` 用 c_int 接收）。
#[repr(C)]
pub struct AmCallResult {
    pub struct_size: u32,
    pub data_json: *const c_char,
    pub state_hints: *const *const c_char,
    pub state_hints_len: usize,
    pub status: c_int,
    pub state_resource: *const c_char,
    pub summary: *const c_char,
    /// 内容注解（MCP `Annotations`，JSON 对象文本）；NULL = 无。
    pub annotations_json: *const c_char,
}

#[repr(C)]
pub struct AmResourceSpec {
    pub name: *const c_char,
    pub description: *const c_char,
    pub mime_type: *const c_char,
}

impl Default for AmLifecycle {
    fn default() -> Self {
        let d = LifecyclePolicy::default();
        Self {
            mode: 0,
            idle_timeout_ms: d.idle_timeout_ms,
            hidden_idle_timeout_ms: d.hidden_idle_timeout_ms,
            grace_ms: d.grace_ms,
            residency: 0,
            wake_kind: -1,
            wake_target: std::ptr::null(),
            wake_background: false,
        }
    }
}
