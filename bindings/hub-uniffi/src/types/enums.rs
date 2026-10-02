//! 枚举：风险、暴露方式、协议模式、唤醒、可用性、工具格式等，及与 `app_mcp_hub` 枚举的映射。

use app_mcp_hub as hub;

/// 风险等级。顺序：read < write < destructive < payment < os-sensitive。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum Risk {
    Read,
    Write,
    Destructive,
    Payment,
    OsSensitive,
}

/// 工具暴露方式（spec/hub-api.md 3.7）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum ToolExposure {
    /// 列出全部工具。
    All,
    /// 工具列表只含 `apps.*` 与本会话展开过（`apps.tools`）、调用过或选定了实例的 App 的工具。
    Progressive,
    /// App 与上游工具总数超过 `tool_exposure_threshold` 时按 `Progressive`，否则按 `All`（默认）。
    Auto,
}

impl From<ToolExposure> for hub::ToolExposure {
    fn from(v: ToolExposure) -> Self {
        match v {
            ToolExposure::All => hub::ToolExposure::All,
            ToolExposure::Progressive => hub::ToolExposure::Progressive,
            ToolExposure::Auto => hub::ToolExposure::Auto,
        }
    }
}

/// MCP 出口协商的协议版本范围（spec/hub-api.md 3.6「协议版本」）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum McpProtocolMode {
    /// 默认：`initialize` 客户端走 legacy 会话，每请求自带 `_meta` 的客户端可协商 2026-07-28（`subscriptions/listen` 可用）。
    Auto,
    /// 回退开关：只声明到 2025-11-25，`subscriptions/listen` 不可用。
    LegacyOnly,
}

impl From<McpProtocolMode> for hub::McpProtocolMode {
    fn from(v: McpProtocolMode) -> Self {
        match v {
            McpProtocolMode::Auto => hub::McpProtocolMode::Auto,
            McpProtocolMode::LegacyOnly => hub::McpProtocolMode::LegacyOnly,
        }
    }
}

/// 唤醒器配置（spec/hub-api.md 3.5；对应 JSON 的 `"system"` / `"none"` / `{"exec": [...]}`）。
/// `set_waker` 设置的实现优先。
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum WakerConfig {
    /// 按平台执行系统激活（默认）。
    System,
    /// 不唤醒：休眠实例 / 未运行 App 的调用直接返回 `APP_DISCONNECTED`（JSON 中为 `"none"`）。
    Disabled,
    /// 执行 `argv[0] argv[1..]`（不经 shell），唤醒请求以一行 JSON 写入其 stdin。
    Exec { argv: Vec<String> },
}

impl From<WakerConfig> for hub::WakerConfig {
    fn from(v: WakerConfig) -> Self {
        match v {
            WakerConfig::System => hub::WakerConfig::System,
            WakerConfig::Disabled => hub::WakerConfig::None,
            WakerConfig::Exec { argv } => hub::WakerConfig::Exec(argv),
        }
    }
}

/// 调用时 App 需要的激活方式。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum Activation {
    Headless,
    Background,
    Foreground,
}

/// 实例可见性。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum Visibility {
    Visible,
    Hidden,
    Frozen,
}

/// App 的来源。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum AppKind {
    /// 通过 App 端 SDK 连接（或只有静态清单）的 App。
    App,
    /// Hub 以子进程启动的上游 MCP 服务器。
    Upstream,
}

/// 工具当前是否可调用。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum Availability {
    /// 至少一个已连接实例注册了该工具。
    Available,
    /// App 未连接，工具来自静态清单。
    Disconnected,
    /// App 已连接，但没有实例注册该静态工具。
    NotRegistered,
    /// 只由休眠实例提供（来自休眠前的快照）；调用时 Hub 先唤醒实例再派发。
    Dormant,
    /// 其他不可直接调用的状态（兜底：Hub 未来新增、本绑定尚未单独映射的可用性）。
    Unavailable,
}

/// 唤醒方式（spec/lifecycle.md 第 5 节）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum WakeKind {
    /// 自定义 URL scheme：`<scheme>://app-mcp/wake?token=`。
    Uri,
    /// Windows 打包应用（AUMID）。
    Aumid,
    /// macOS Apple Event（bundle id）。
    AppleEvent,
    /// Linux D-Bus `org.freedesktop.Application.ActivateAction`。
    Dbus,
    /// Android 显式广播（`target` 为组件名，如 `com.example/.WakeReceiver`）。
    AndroidIntent,
    /// 网页：打开 / 聚焦 URL。
    WebUrl,
    /// 不可唤醒。
    None,
}

impl From<hub::WakeKind> for WakeKind {
    fn from(k: hub::WakeKind) -> Self {
        #[allow(unreachable_patterns)]
        match k {
            hub::WakeKind::Uri => WakeKind::Uri,
            hub::WakeKind::Aumid => WakeKind::Aumid,
            hub::WakeKind::AppleEvent => WakeKind::AppleEvent,
            hub::WakeKind::Dbus => WakeKind::Dbus,
            hub::WakeKind::AndroidIntent => WakeKind::AndroidIntent,
            hub::WakeKind::WebUrl => WakeKind::WebUrl,
            _ => WakeKind::None,
        }
    }
}

/// 工具定义 / 调用格式（spec/hub-api.md 第 5 节）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum ToolFormat {
    Mcp,
    OpenAiChat,
    OpenAiResponses,
    Anthropic,
    Gemini,
}

enum_map!(Risk <=> hub::Risk { Read, Write, Destructive, Payment, OsSensitive });
enum_map!(Activation <=> hub::Activation { Headless, Background, Foreground });
enum_map!(Visibility <=> hub::Visibility { Visible, Hidden, Frozen });
enum_map!(AppKind <=> hub::AppKind { App, Upstream });

impl From<hub::Availability> for Availability {
    fn from(v: hub::Availability) -> Self {
        #[allow(unreachable_patterns)]
        match v {
            hub::Availability::Available => Availability::Available,
            hub::Availability::Disconnected => Availability::Disconnected,
            hub::Availability::NotRegistered => Availability::NotRegistered,
            hub::Availability::Dormant => Availability::Dormant,
            // 兜底：Hub 新增的可用性变体。
            _ => Availability::Unavailable,
        }
    }
}
enum_map!(ToolFormat <=> hub::ToolFormat { Mcp, OpenAiChat, OpenAiResponses, Anthropic, Gemini });

/// App 工具对界面的依赖（spec/protocol.md 3.4）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum ToolSurface {
    /// 不依赖界面，App 在后台也可调用（未声明即此值）。
    App,
    /// 只在所在界面可见且处于最上层时注册。
    View,
}

enum_map!(ToolSurface <=> hub::ToolSurface { App, View });

/// 结果与其 `outputSchema` 不符时 Hub 的处理（spec/hub-api.md 3.11）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum OutputValidation {
    /// 不校验。
    Off,
    /// 校验，不符时只记日志（默认）。
    Log,
    /// 校验，不符时调用以 `HANDLER_ERROR` 结束。
    Reject,
}

enum_map!(OutputValidation <=> hub::OutputValidation { Off, Log, Reject });

/// 调用结果的业务状态（spec/protocol.md 3.2）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum ResultStatus {
    /// 已完成（缺省）。
    Done,
    /// 已受理、尚未完成（等待用户在 App 内确认或异步处理）；后续状态见 `CallOutcome.state_resource`。
    Pending,
    /// 只完成了一部分，说明见 `CallOutcome.summary`。
    Partial,
    /// 没有做任何改动。
    Noop,
}

enum_map!(ResultStatus <=> hub::ResultStatus { Done, Pending, Partial, Noop });

/// 内容面向的对象（MCP 内容注解 `audience`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum Audience {
    User,
    Assistant,
}

enum_map!(Audience <=> hub::Audience { User, Assistant });
