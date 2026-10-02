//! 策略挂点（spec/hub-api.md 3.13）：规则、配置与加载状态。

use app_mcp_hub as hub;

/// 策略规则的动作。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum PolicyAction {
    /// 不出现在任何列表中，调用按 `TOOL_NOT_FOUND`（资源读取按 `RESOURCE_NOT_FOUND`）。
    Hide,
    /// 可见，在 `hooks` 指定的执行点以 `POLICY_DENIED` 拒绝。
    Deny,
}

enum_map!(PolicyAction <=> hub::PolicyAction { Hide, Deny });

/// 策略执行点。规则的 `hooks` 只能写 `Call` / `Wake`（`List` 只由 `Hide` 隐式使用，`Handle` 尚未实现，写了校验报错）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum PolicyHook {
    List,
    Call,
    Wake,
    Handle,
}

enum_map!(PolicyHook <=> hub::PolicyHook { List, Call, Wake, Handle });

/// 按 App 声明的 MCP 注解匹配：给出的每一项都与工具注解相等才命中；工具未声明该项时不命中。至少给出一项。
#[derive(Clone, Debug, Default, PartialEq, Eq, uniffi::Record)]
pub struct AnnotationMatch {
    #[uniffi(default = None)]
    pub read_only_hint: Option<bool>,
    #[uniffi(default = None)]
    pub destructive_hint: Option<bool>,
    #[uniffi(default = None)]
    pub idempotent_hint: Option<bool>,
    #[uniffi(default = None)]
    pub open_world_hint: Option<bool>,
}

/// 一条策略规则（与 JSON 形式 `{"id","action","app","tool"?,"annotations"?,"agent"?,"hooks"?}` 同构）。
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct PolicyRule {
    /// 规则标识：`[A-Za-z0-9_.-]{1,64}`，在规则集中唯一；`POLICY_DENIED` 的 `details.ruleId`。
    pub id: String,
    pub action: PolicyAction,
    /// appId（或上游名）模式：精确名，或以 `*` 结尾的前缀；`*` 匹配全部。
    pub app: String,
    /// 工具局部名模式（不含 appId），规则同 `app`。与 `annotations` 都为空时作用于整个 App。
    #[uniffi(default = None)]
    pub tool: Option<String>,
    #[uniffi(default = None)]
    pub annotations: Option<AnnotationMatch>,
    /// `Deny` 的执行点：`Call` / `Wake` 的非空子集，为空时为 `[Call]`。`Hide` 不能给出。
    #[uniffi(default = None)]
    pub hooks: Option<Vec<PolicyHook>>,
    /// 只对该 Agent（第 16 项 N5 登记的名字，规则同 `app`）发起的操作生效；只能用于 `Deny`。放在最后：已有的位置参数写法不变。
    #[uniffi(default = None)]
    pub agent: Option<String>,
}

/// 策略规则集；按顺序匹配，`Deny` 取第一条命中的规则。空规则集 = 不做任何限制（默认）。
#[derive(Clone, Debug, Default, PartialEq, Eq, uniffi::Record)]
pub struct PolicyConfig {
    #[uniffi(default = [])]
    pub rules: Vec<PolicyRule>,
}

impl From<AnnotationMatch> for hub::AnnotationMatch {
    fn from(m: AnnotationMatch) -> Self {
        hub::AnnotationMatch {
            read_only_hint: m.read_only_hint,
            destructive_hint: m.destructive_hint,
            idempotent_hint: m.idempotent_hint,
            open_world_hint: m.open_world_hint,
        }
    }
}

impl From<hub::AnnotationMatch> for AnnotationMatch {
    fn from(m: hub::AnnotationMatch) -> Self {
        AnnotationMatch {
            read_only_hint: m.read_only_hint,
            destructive_hint: m.destructive_hint,
            idempotent_hint: m.idempotent_hint,
            open_world_hint: m.open_world_hint,
        }
    }
}

impl From<PolicyRule> for hub::PolicyRule {
    fn from(r: PolicyRule) -> Self {
        hub::PolicyRule {
            id: r.id,
            action: r.action.into(),
            app: r.app,
            tool: r.tool,
            annotations: r.annotations.map(Into::into),
            agent: r.agent,
            hooks: r.hooks.map(|h| h.into_iter().map(Into::into).collect()),
        }
    }
}

impl From<hub::PolicyRule> for PolicyRule {
    fn from(r: hub::PolicyRule) -> Self {
        PolicyRule {
            id: r.id,
            action: r.action.into(),
            app: r.app,
            tool: r.tool,
            annotations: r.annotations.map(Into::into),
            agent: r.agent,
            hooks: r.hooks.map(|h| h.into_iter().map(Into::into).collect()),
        }
    }
}

impl From<PolicyConfig> for hub::PolicyConfig {
    fn from(c: PolicyConfig) -> Self {
        hub::PolicyConfig { rules: c.rules.into_iter().map(Into::into).collect() }
    }
}

/// 一条生效的规则及其命中次数。
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct PolicyRuleStatus {
    pub rule: PolicyRule,
    /// 自本规则集生效以来，该规则拒绝或按不存在处理的调用 / 唤醒次数（列表过滤不计）。
    pub hits: u64,
}

/// 最近一次加载规则失败（之前的规则继续生效）。
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct PolicyLoadError {
    pub message: String,
    /// Unix 毫秒。
    pub at_ms: u64,
}

/// 策略状态（`AppMcpHub::policy()`、`HubStatus.policy`）。
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct PolicyStatus {
    /// 生效的规则（按顺序）与命中次数。
    pub rules: Vec<PolicyRuleStatus>,
    /// 当前规则集生效的时刻（Unix 毫秒）。
    pub loaded_at_ms: u64,
    /// 最近一次 `set_policy` 失败的原因；之后成功加载时清除。
    #[uniffi(default = None)]
    pub last_error: Option<PolicyLoadError>,
}

impl From<hub::PolicyStatus> for PolicyStatus {
    fn from(s: hub::PolicyStatus) -> Self {
        PolicyStatus {
            rules: s
                .rules
                .into_iter()
                .map(|r| PolicyRuleStatus { rule: r.rule.into(), hits: r.hits })
                .collect(),
            loaded_at_ms: s.loaded_at_ms,
            last_error: s.last_error.map(|e| PolicyLoadError { message: e.message, at_ms: e.at_ms }),
        }
    }
}
