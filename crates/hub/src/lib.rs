//! app-mcp-hub：Agent 端 Hub 库（spec/hub-api.md）。
//!
//! 把“连接本机所有 App”的能力嵌进厂商自己的 Agent / 助手：列工具、调用、读资源、事件、
//! 审批与配对回调、按 LLM 厂商格式导出工具并分派调用；也可同时以 MCP（stdio / Streamable HTTP）对外提供。
//!
//! - [`Hub`]：入口（[`Hub::start`]）。
//! - [`format`]：工具格式导出与分派（[`ToolFormat`]、[`format::NameCodec`]）。
//! - [`McpSession`]：MCP 出口（rmcp `ServerHandler`），与 [`Hub::call_tool`] 共用一份调用逻辑。
//! - 其余模块（注册表、路由、App 连接服务、上游聚合、总览）为内部实现，公开以便测试与高级用法。
//!
//! 可执行程序 `app-mcp-host` 是本库之上的命令行薄壳。

mod activity;
mod agent_control;
pub mod app_server;
pub mod call;
pub mod connection;
pub mod connector;
pub mod dormant_store;
pub mod features;
pub mod format;
mod heap;
pub mod http_server;
pub mod hub;
mod instance;
mod ipc;
mod lease;
mod lifecycle;
mod limits;
mod mcp_convert;
mod navigate;
pub mod names;
mod naming;
pub mod pages;
pub mod policy;
mod power;
pub mod progress;
#[cfg(feature = "mcp-server")]
pub mod mcp;
pub mod origin;
pub mod overview;
pub mod registry;
mod request_meta;
pub mod routing;
pub mod schema;
mod task;
pub mod tool_def;
pub mod types;
pub mod upstream;
pub mod wake;

pub use app_mcp_protocol::{
    Activation, Audience, ContentAnnotations, ErrorKind, LifecycleMode, ResultStatus, Risk, ToolAnnotations, ToolError,
    ToolSurface, Visibility,
};
pub use dormant_store::{DormantStoreStatus, StoreFileInfo, StoreIssue};
pub use format::ToolFormat;
pub use lease::{LeaseOverrides, LeasePairStatus, LeasePolicy, LeaseStatus};
pub use limits::{LimitOverrides, LimitPolicy, OutputValidation, RateLimit};
pub use activity::PreboundListeners;
pub use http_server::{Health, HttpOptions};
pub use progress::ProgressUpdate;
pub use policy::{
    AnnotationMatch, MAX_POLICY_RULES, PolicyAction, PolicyConfig, PolicyHook, PolicyLoadError, PolicyRule, PolicyRuleStatus,
    PolicyStatus,
};
pub use connector::{BlockedTarget, ConnectorError, Connector, DialedChannel, DiscoveredName, NameEvent};
pub use hub::{
    DEFAULT_CHANNEL_GRACE, DEFAULT_NAVIGATE_TIMEOUT, DEFAULT_PRINCIPAL_SELECT_TTL, DEFAULT_PROGRESS_INTERVAL, DEFAULT_STATELESS_LIST_TTL, DEFAULT_TASK_IDLE_TTL, DEFAULT_TOOL_EXPOSURE_THRESHOLD, DEFAULT_WAKE_RATE_LIMIT, Hub, HubConfig, LocalAppChannel, RESOURCE_URI_SCHEME,
    load_manifests, parse_resource_uri, resource_uri,
};
#[cfg(feature = "mcp-server")]
pub use mcp::McpSession;
pub use types::{
    AppInfo, AppKind, AppOverviewInfo, AppState, AppStatus, ApprovalHandler, ApprovalPolicy,
    AgentTaskStatus, ApprovalRequest, AuthStatus, Availability, AwakeReason, CallOutcome, CallRequest, DiagnosticReport, HubError,
    HubEvent, HubResource, HubStatus, HubTool, InstanceInfo, InstancePower, InstanceState, InstanceStatus, LastError,
    PairingHandler, PairingRequest, ResourceContent, TaskLeaseStatus, TaskSelectionStatus, ToolDeclaration, ToolExposure,
    ToolFilter, risk_rank,
};
pub use task::CallerKind;
pub use upstream::UpstreamConfig;
pub use wake::{
    ExecWaker, Platform, SystemWaker, WakeAction, WakeCommand, WakeDescriptor, WakeKind, WakeRequest,
    Waker, WakerConfig,
};

/// Windows `CREATE_NO_WINDOW`：Hub 启动的子进程（唤醒命令、上游）不创建控制台窗口。
#[cfg(windows)]
pub(crate) const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 供实现 [`ApprovalHandler`] / [`PairingHandler`] 使用（`#[app_mcp_hub::async_trait]`）。
pub use async_trait::async_trait;
