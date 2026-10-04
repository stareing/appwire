//! JS 对象（`#[napi(object)]`）：客户端配置、工具 / 资源定义、调用结果与事件，以及到原生类型的转换。

use app_mcp_native as native;
use napi_derive::napi;
use native::{ContentAnnotations, LifecyclePolicy, StateInfo, ToolAnnotations, WakeDescriptor};

use super::convert::{
    parse_activation, parse_audience, parse_lifecycle_mode, parse_millis, parse_residency, parse_result_status,
    parse_risk, parse_surface, parse_wake_kind, status_str,
};

/// 本绑定抛出的错误：`status` 字符串成为 JS 错误的 `code`（napi-derive 按名称 `Result` 识别返回类型）。
use napi::Result;

/// `new NativeClient(config, listener?)` 的配置。
#[napi(object)]
pub struct ClientConfig {
    pub app_id: String,
    pub app_name: String,
    pub instance_id: Option<String>,
    /// `'native'`（默认）| `'hybrid'` | `'web'`。
    pub client_kind: Option<String>,
    pub host_url: Option<String>,
    pub app_version: Option<String>,
    pub instance_title: Option<String>,
    pub token: Option<String>,
    pub launch_token: Option<String>,
    pub max_concurrent_calls: Option<u32>,
    /// 排队中的调用上限（spec/protocol.md 5.3），缺省 64；0 表示不限。
    pub max_queued_calls: Option<u32>,
    /// 用户正在操作（`setBusy`）期间写调用的处理方式（spec/protocol.md 5.3）：`'reject'`（默认）| `'queue'`。
    pub busy_policy: Option<String>,
    /// App 总览（spec/protocol.md 第 7 节）。
    pub overview: Option<OverviewInit>,
    /// 生命周期策略（spec/lifecycle.md 第 3 节）。缺省 `persistent`。
    pub lifecycle: Option<LifecycleInit>,
    /// 建立 WebSocket 连接的超时（毫秒），默认 5000。
    pub connect_timeout_ms: Option<u32>,
    /// 心跳策略（spec/lifecycle.md 第 11 节）：`'auto'`（默认）| `'always'` | `'off'`。
    pub heartbeat: Option<String>,
    /// 调用去重（spec/protocol.md 3.3）：同一 `callId` 在有效期内重放首次结果。缺省保留 5 分钟、最多 64 条。
    pub call_dedup: Option<CallDedupInit>,
    /// 按名寻址（spec/naming.md）：`start()` 后在系统名字服务登记本 App，Hub 按名拨入（进程未运行时由系统激活）。
    /// Linux：D-Bus 会话总线名 `dev.appmcp.App.<appId>`；Windows：命名管道 `\\.\pipe\appmcp-<用户 SID>-<appId>`；
    /// 两者都需先 `app-mcp-host app install` 登记。其他平台不支持（经日志事件报告，其余照常）。默认 `false`。
    pub register_name: Option<bool>,
    /// 登记实例名（`[a-z][a-z0-9-]{0,31}`，不能是 `default`）：另登记 `dev.appmcp.App.<appId>.<instance>`
    /// （Windows 管道 `…-<appId>.<instance>`），供 `appmcp://<appId>/<instance>` 寻址。不合法时构造抛错。
    pub name_instance: Option<String>,
}

/// 调用去重策略。未提供的字段取默认值（`ttlMs` 300000、`maxEntries` 64）；任一为 0 关闭去重。
#[napi(object)]
pub struct CallDedupInit {
    pub ttl_ms: Option<f64>,
    pub max_entries: Option<u32>,
}

impl CallDedupInit {
    pub(super) fn into_policy(self) -> Result<native::CallDedupPolicy, String> {
        let mut p = native::CallDedupPolicy::default();
        if let Some(v) = self.ttl_ms {
            p.ttl_ms = parse_millis("callDedup.ttlMs", v)?;
        }
        if let Some(n) = self.max_entries {
            p.max_entries = n as usize;
        }
        Ok(p)
    }
}

/// 生命周期策略。未提供的字段取默认值（见 spec/lifecycle.md 第 3 节）。
#[napi(object)]
pub struct LifecycleInit {
    /// `'persistent'`（默认）| `'idle'` | `'on-demand'`。
    pub mode: Option<String>,
    pub idle_timeout_ms: Option<f64>,
    pub hidden_idle_timeout_ms: Option<f64>,
    pub grace_ms: Option<f64>,
    /// `'keep'`（默认）| `'exit-when-idle'` | `'exit-always'`。
    pub residency: Option<String>,
    /// 本实例的唤醒描述，随 `app/sleep` 上报。
    pub wake: Option<WakeInit>,
    /// `idle` / `on-demand` 下连续多少次"Host 不在"后转休眠；默认 3，0 = 一直重连（spec/lifecycle.md 第 11 节）。
    pub host_absent_retries: Option<u32>,
    /// 回退到 4e 之前的定时器行为（串行租约、无限重连、双向心跳、调用后按空闲时长、任何订阅都阻止休眠）。默认 `false`。
    pub legacy_timers: Option<bool>,
    /// 调用 / 资源读取后的合并窗口（spec/lifecycle.md 第 13 节 B1）。默认 2000。
    pub merge_window_ms: Option<f64>,
    /// `idle` / `on-demand` 下进入后台（可见 → 隐藏 / 冻结）且空闲时立即休眠，不等租约（B4）。默认 `false`。
    pub sleep_on_background: Option<bool>,
}

/// 唤醒描述（spec/lifecycle.md 第 5 节）。
#[napi(object)]
pub struct WakeInit {
    /// `'uri' | 'aumid' | 'apple-event' | 'dbus' | 'android-intent' | 'web-url' | 'none'`。
    pub kind: String,
    pub target: Option<String>,
    pub background: Option<bool>,
}

impl LifecycleInit {
    pub(super) fn into_policy(self) -> Result<LifecyclePolicy, String> {
        let mut p = LifecyclePolicy::default();
        if let Some(mode) = self.mode.as_deref() {
            p.mode = parse_lifecycle_mode(mode)?;
        }
        if let Some(v) = self.idle_timeout_ms {
            p.idle_timeout_ms = parse_millis("lifecycle.idleTimeoutMs", v)?;
        }
        if let Some(v) = self.hidden_idle_timeout_ms {
            p.hidden_idle_timeout_ms = parse_millis("lifecycle.hiddenIdleTimeoutMs", v)?;
        }
        if let Some(v) = self.grace_ms {
            p.grace_ms = parse_millis("lifecycle.graceMs", v)?;
        }
        if let Some(r) = self.residency.as_deref() {
            p.residency = parse_residency(r)?;
        }
        if let Some(w) = self.wake {
            p.wake = Some(WakeDescriptor {
                kind: parse_wake_kind(&w.kind)?,
                target: w.target,
                background: w.background.unwrap_or(false),
            });
        }
        if let Some(n) = self.host_absent_retries {
            p.host_absent_retries = n;
        }
        if let Some(b) = self.legacy_timers {
            p.legacy_timers = b;
        }
        if let Some(v) = self.merge_window_ms {
            p.merge_window_ms = parse_millis("lifecycle.mergeWindowMs", v)?;
        }
        if let Some(b) = self.sleep_on_background {
            p.sleep_on_background = b;
        }
        Ok(p)
    }
}

/// App 总览：`summary` 一句话简介，`body` Markdown 正文，`locale` 如 `'zh-CN'`。
#[napi(object)]
pub struct OverviewInit {
    pub summary: String,
    pub body: Option<String>,
    pub locale: Option<String>,
}

impl From<OverviewInit> for native::AppOverview {
    fn from(o: OverviewInit) -> Self {
        native::AppOverview { summary: o.summary, body: o.body, locale: o.locale }
    }
}

/// 连接状态。`status` 与 `@app-mcp/web` 的 `ConnectionState.status` 取值一致。
#[napi(object)]
pub struct JsStateInfo {
    pub status: String,
    pub retry_in_ms: Option<f64>,
    pub reason: Option<String>,
    /// 与 `reason` 对应的错误码（spec/protocol.md 10.1）。
    pub code: Option<String>,
}

impl From<StateInfo> for JsStateInfo {
    fn from(s: StateInfo) -> Self {
        Self {
            status: status_str(s.status).to_string(),
            retry_in_ms: s.retry_in_ms.map(|v| v as f64),
            reason: s.reason,
            code: s.code,
        }
    }
}

/// 客户端事件：`type` 为 `'state'`（带 `state`）、`'paired'`（带 `token`）、`'log'`（带 `level`、`message`）
/// 或 `'idle-exit'`（已休眠且驻留策略允许退出进程，无其他字段）。
#[napi(object)]
pub struct ClientEvent {
    #[napi(js_name = "type")]
    pub kind: String,
    pub state: Option<JsStateInfo>,
    pub token: Option<String>,
    pub level: Option<String>,
    pub message: Option<String>,
}

/// 工具定义。未提供的字段取默认值：无参数、risk = write、enabled = true。
#[napi(object)]
pub struct ToolSpecInit {
    pub name: String,
    pub description: String,
    /// JSON Schema 文本（顶层 `type` 为 `"object"`）。
    pub input_schema_json: Option<String>,
    pub risk: Option<String>,
    pub activation: Option<String>,
    pub title: Option<String>,
    pub enabled: Option<bool>,
    /// 标准 MCP 工具注解（与旧写法 `risk` 同时给出时声明的字段逐个优先）。
    pub annotations: Option<ToolAnnotationsInit>,
    /// 结果的 JSON Schema 文本（MCP `outputSchema`，根类型不限）。
    pub output_schema_json: Option<String>,
    /// `'app'`（缺省）/ `'view'`：对界面的依赖（spec/protocol.md 3.4）。
    pub surface: Option<String>,
    /// 所在页面名；Hub 在该工具未注册时据此导航。
    pub page: Option<String>,
    /// 后台替代（spec/protocol.md 3.4）：同一 App 中一个 `app` 工具的局部名；本工具因 App 在后台不可调用时 Hub 改调它。
    pub background_tool: Option<String>,
    /// 本工具同时执行的调用上限（spec/protocol.md 5.3）；缺省 / 0 = 不单独限制。
    pub concurrency: Option<u32>,
    /// 互斥组（spec/protocol.md 5.3）：同组的工具同一时刻至多一个在执行。
    pub exclusive: Option<String>,
    /// 实现的标准意图（spec/intents.md），如 `["message.send@1"]`。
    pub implements: Option<Vec<String>>,
}

/// 标准 MCP 工具注解（spec/protocol.md 第 3 节）。
#[napi(object)]
pub struct ToolAnnotationsInit {
    pub title: Option<String>,
    pub read_only_hint: Option<bool>,
    pub destructive_hint: Option<bool>,
    pub idempotent_hint: Option<bool>,
    pub open_world_hint: Option<bool>,
}

impl From<ToolAnnotationsInit> for ToolAnnotations {
    fn from(a: ToolAnnotationsInit) -> Self {
        ToolAnnotations {
            title: a.title,
            read_only_hint: a.read_only_hint,
            destructive_hint: a.destructive_hint,
            idempotent_hint: a.idempotent_hint,
            open_world_hint: a.open_world_hint,
        }
    }
}

/// 标准 MCP 内容注解（调用结果的 `annotations`）。
#[napi(object)]
pub struct ContentAnnotationsInit {
    /// `'user'` / `'assistant'`。
    pub audience: Option<Vec<String>>,
    /// 0（可选）到 1（必需）。
    pub priority: Option<f64>,
    /// ISO 8601。
    pub last_modified: Option<String>,
}

impl ContentAnnotationsInit {
    pub(super) fn into_annotations(self) -> Result<ContentAnnotations, String> {
        let audience = self
            .audience
            .map(|roles| roles.iter().map(|r| parse_audience(r)).collect::<Result<Vec<_>, String>>())
            .transpose()?;
        Ok(ContentAnnotations { audience, priority: self.priority, last_modified: self.last_modified })
    }
}

/// 调用成功的完整结果（`Call.completeWith`）。缺省 = 无返回值、`done`。
#[napi(object)]
pub struct CallResultInit {
    /// 返回值 JSON 文本；`null` / 省略表示无返回值。
    pub data_json: Option<String>,
    pub state_hints: Option<Vec<String>>,
    /// `'done'`（缺省）/ `'pending'` / `'partial'` / `'noop'`。
    pub status: Option<String>,
    /// `pending` 时可读取后续状态的资源名。
    pub state_resource: Option<String>,
    /// 一句面向模型 / 用户的结论。
    pub summary: Option<String>,
    pub annotations: Option<ContentAnnotationsInit>,
}

impl CallResultInit {
    pub(super) fn into_result(self) -> Result<native::CallResult, String> {
        Ok(native::CallResult {
            data_json: self.data_json,
            state_hints: self.state_hints.unwrap_or_default(),
            status: self.status.as_deref().map(parse_result_status).transpose()?.unwrap_or_default(),
            state_resource: self.state_resource,
            summary: self.summary,
            annotations: self.annotations.map(ContentAnnotationsInit::into_annotations).transpose()?,
        })
    }
}

impl ToolSpecInit {
    /// 拆成定义与选项（选项中未给出的字段为 `None`）。
    pub(super) fn into_parts(mut self) -> Result<(native::ToolSpec, native::ToolOptions), String> {
        let options = native::ToolOptions {
            annotations: self.annotations.take().map(ToolAnnotations::from),
            output_schema_json: self.output_schema_json.take(),
            surface: self.surface.take().as_deref().map(parse_surface).transpose()?.unwrap_or_default(),
            page: self.page.take(),
            background_tool: self.background_tool.take(),
            concurrency: self.concurrency.take().unwrap_or(0),
            exclusive: self.exclusive.take(),
            implements: self.implements.take().unwrap_or_default(),
            cache: None,
        };
        Ok((self.into_spec()?, options))
    }

    pub(super) fn into_spec(self) -> Result<native::ToolSpec, String> {
        let mut spec = native::ToolSpec::new(self.name, self.description);
        spec.input_schema_json = self.input_schema_json;
        if let Some(risk) = self.risk.as_deref() {
            spec.risk = parse_risk(risk)?;
        }
        spec.activation = self.activation.as_deref().map(parse_activation).transpose()?;
        spec.title = self.title;
        spec.enabled = self.enabled.unwrap_or(true);
        Ok(spec)
    }
}

#[napi(object)]
pub struct ResourceSpecInit {
    pub name: String,
    pub description: String,
    pub mime_type: Option<String>,
    /// 需实时推送（spec/lifecycle.md 第 13 节 B3）：被订阅时保持连接、休眠中变化时回连推送。默认 `false`。
    pub realtime: Option<bool>,
    /// 资源内容的标注（MCP 内容注解），Hub 放到 MCP `resources/list` 的资源注解上。缺省未声明。
    pub annotations: Option<ContentAnnotationsInit>,
}

impl ResourceSpecInit {
    /// 拆成定义与选项；`annotations.audience` 取值不合法时返回 `INVALID_ARG`。
    pub(super) fn into_parts(self) -> Result<(native::ResourceSpec, native::ResourceOptions), String> {
        let options = native::ResourceOptions {
            realtime: self.realtime.unwrap_or(false),
            annotations: self.annotations.map(ContentAnnotationsInit::into_annotations).transpose()?,
            cache: None,
        };
        let spec = native::ResourceSpec { name: self.name, description: self.description, mime_type: self.mime_type };
        Ok((spec, options))
    }
}

/// 事件声明（spec/protocol.md 3.5）。
#[napi(object)]
pub struct EventSpecInit {
    pub name: String,
    pub description: String,
    /// 载荷的 JSON Schema 文本（描述用，Hub 不校验）；省略 = 不描述。
    pub payload_schema_json: Option<String>,
}

impl EventSpecInit {
    /// @error `payloadSchemaJson` 不是合法 JSON → `INVALID_JSON`。
    pub(super) fn into_info(self) -> Result<native::EventInfo, String> {
        let payload_schema = self
            .payload_schema_json
            .as_deref()
            .map(|text| {
                serde_json::from_str(text)
                    .map_err(|e| napi::Error::new("INVALID_JSON".to_string(), format!("事件载荷 schema 不是合法 JSON：{e}")))
            })
            .transpose()?;
        Ok(native::EventInfo { name: self.name, description: self.description, payload_schema })
    }
}
