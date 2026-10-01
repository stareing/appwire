//! 策略挂点（spec/hub-api.md 3.13；docs/plans/16-agent-os.md P2、18-user-loop.md L5）。
//!
//! 类比 LSM：本库只提供执行点，不内置任何判断。规则由用户（常驻 Host 的 `<home>/policy.json`）或厂商（`HubConfig.policy`、
//! [`crate::Hub::set_policy`]）写；没有规则时所有执行点直接放行，行为与没有本模块时完全一致。
//!
//! - **执行点**（[`PolicyHook`]）：列出（`tools/list`、`apps.*`、资源列表、总览）、调用、唤醒；句柄访问只定义类型，尚未接入。
//! - **动作**（[`PolicyAction`]）：`hide`（不出现在任何列表中，调用按 `TOOL_NOT_FOUND`，全局生效）与
//!   `deny`（可见，在指定执行点以 `POLICY_DENIED` 拒绝，附命中规则的 `id`，不附规则内容）。
//! - **匹配**：App（`appId` 或上游名）、工具局部名（都支持 `*` 后缀通配）、App 声明的 MCP 注解（Agent 实际看到的注解，
//!   只有声明了的值参与匹配）。按 Agent 区分的规则依赖任务对象（P1），尚未支持。
//!
//! 本模块是纯数据与纯函数（不做 I/O、不读时钟）；执行点的调用在 [`crate::call`] 与 [`crate::hub`]。

use app_mcp_protocol::{ErrorKind, ToolAnnotations, ToolError};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// 规则 `id` 的最大长度。
const MAX_RULE_ID_LEN: usize = 64;

/// 名称模式（`app` / `tool`）的最大长度。
const MAX_PATTERN_LEN: usize = 128;

/// 规则条数上限（B-07）。
pub const MAX_POLICY_RULES: usize = 1024;

/// 通配符：只能出现在模式末尾，表示前缀匹配；单独的 `*` 匹配全部。
const WILDCARD: char = '*';

/// 规则动作。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PolicyAction {
    /// 不出现在任何列表中，调用按 `TOOL_NOT_FOUND`（资源读取按 `RESOURCE_NOT_FOUND`）。全局生效。
    Hide,
    /// 可见，在 `hooks` 指定的执行点以 `POLICY_DENIED` 拒绝。
    Deny,
}

/// 执行点。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PolicyHook {
    /// 列出：MCP `tools/list`、`resources/list`、`apps.*`、总览与 Hub API 的查询（只由 `hide` 使用）。
    List,
    /// 调用：名称解析之后、资源保护（限流 / 大小上限）与审批之前。
    Call,
    /// 唤醒：休眠 / 未运行的 App 被唤醒之前（调用与资源读取触发的唤醒）。
    Wake,
    /// 句柄访问（第 17 项数据句柄）：只定义执行点，尚未接入；规则写 `handle` 时校验报错。
    Handle,
}

impl PolicyHook {
    pub fn as_str(self) -> &'static str {
        match self {
            PolicyHook::List => "list",
            PolicyHook::Call => "call",
            PolicyHook::Wake => "wake",
            PolicyHook::Handle => "handle",
        }
    }
}

/// 按 App 声明的 MCP 注解匹配：列出的每一项都与工具的注解（Agent 实际看到的，`HubTool.annotations`）相等才算命中；
/// 工具没有声明该项时不命中（本库不按 MCP 缺省值或 `risk` 以外的信息推断）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnnotationMatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_only_hint: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destructive_hint: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotent_hint: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_world_hint: Option<bool>,
}

impl AnnotationMatch {
    fn is_empty(&self) -> bool {
        self.read_only_hint.is_none()
            && self.destructive_hint.is_none()
            && self.idempotent_hint.is_none()
            && self.open_world_hint.is_none()
    }

    fn matches(&self, a: Option<&ToolAnnotations>) -> bool {
        let Some(a) = a else {
            return false;
        };
        let want = |want: Option<bool>, got: Option<bool>| want.is_none_or(|w| got == Some(w));
        want(self.read_only_hint, a.read_only_hint)
            && want(self.destructive_hint, a.destructive_hint)
            && want(self.idempotent_hint, a.idempotent_hint)
            && want(self.open_world_hint, a.open_world_hint)
    }
}

/// 一条规则。JSON 形式见 [`PolicyConfig`]。
///
/// @invariant 作用范围：`tool` 与 `annotations` 都缺省 → 整个 App（`hide` 时 App 本身、全部工具与资源都隐藏）；
/// 否则只作用于匹配的工具。
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PolicyRule {
    /// 规则标识：`[A-Za-z0-9_.-]{1,64}`，在规则集中唯一；`POLICY_DENIED` 只附这个标识。
    pub id: String,
    pub action: PolicyAction,
    /// appId（或上游名）模式：精确名，或以 `*` 结尾的前缀；`*` 匹配全部。
    pub app: String,
    /// 工具局部名模式（不含 appId），规则同 `app`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<AnnotationMatch>,
    /// `deny` 的执行点：`call` / `wake` 的非空子集，缺省 `["call"]`。`hide` 不能写（列表与调用总是一起生效）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hooks: Option<Vec<PolicyHook>>,
}

impl PolicyRule {
    /// 是否作用于整个 App（而不是其中的部分工具）。
    pub fn is_app_level(&self) -> bool {
        self.tool.is_none() && self.annotations.is_none()
    }

    fn denies_at(&self, hook: PolicyHook) -> bool {
        self.action == PolicyAction::Deny
            && self.hooks.as_deref().map_or(hook == PolicyHook::Call, |h| h.contains(&hook))
    }

    /// 是否匹配（App, 工具）。`tool` 为 `None`（不针对具体工具，如资源读取触发的唤醒）时只有 App 级规则匹配。
    fn matches(&self, app_id: &str, tool: Option<(&str, Option<&ToolAnnotations>)>) -> bool {
        if !pattern_matches(&self.app, app_id) {
            return false;
        }
        let Some((name, annotations)) = tool else {
            return self.is_app_level();
        };
        self.tool.as_deref().is_none_or(|p| pattern_matches(p, name))
            && self.annotations.as_ref().is_none_or(|m| m.matches(annotations))
    }

    fn validate(&self) -> Result<(), String> {
        let id = &self.id;
        let id_ok = !id.is_empty()
            && id.len() <= MAX_RULE_ID_LEN
            && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'));
        if !id_ok {
            return Err(format!("规则 id「{id}」不合法：只能包含字母、数字、_ . -，长度 1–{MAX_RULE_ID_LEN}"));
        }
        validate_pattern(&self.app).map_err(|e| format!("规则「{id}」的 app {e}"))?;
        if let Some(t) = &self.tool {
            validate_pattern(t).map_err(|e| format!("规则「{id}」的 tool {e}"))?;
        }
        if self.annotations.as_ref().is_some_and(AnnotationMatch::is_empty) {
            return Err(format!("规则「{id}」的 annotations 为空：至少写一项（readOnlyHint / destructiveHint / idempotentHint / openWorldHint），或去掉该字段"));
        }
        let Some(hooks) = &self.hooks else {
            return Ok(());
        };
        if self.action == PolicyAction::Hide {
            return Err(format!("规则「{id}」：hide 不能写 hooks（列表与调用总是一起隐藏）"));
        }
        if hooks.is_empty() {
            return Err(format!("规则「{id}」的 hooks 为空：写 [\"call\"]、[\"wake\"] 或二者，或去掉该字段（缺省 call）"));
        }
        match hooks.iter().find(|h| !matches!(h, PolicyHook::Call | PolicyHook::Wake)) {
            Some(PolicyHook::List) => Err(format!("规则「{id}」：deny 不能作用于 list（要从列表中去掉请用 hide）")),
            Some(PolicyHook::Handle) => Err(format!("规则「{id}」：执行点 handle（句柄访问）尚未实现")),
            _ => Ok(()),
        }
    }
}

/// 模式匹配：精确相等，或以 `*` 结尾时按前缀匹配。
fn pattern_matches(pattern: &str, name: &str) -> bool {
    match pattern.strip_suffix(WILDCARD) {
        Some(prefix) => name.starts_with(prefix),
        None => pattern == name,
    }
}

fn validate_pattern(p: &str) -> Result<(), String> {
    if p.is_empty() || p.len() > MAX_PATTERN_LEN {
        return Err(format!("「{p}」长度必须为 1–{MAX_PATTERN_LEN}"));
    }
    let body = p.strip_suffix(WILDCARD).unwrap_or(p);
    if body.contains(WILDCARD) {
        return Err(format!("「{p}」不合法：通配符 * 只能出现在末尾"));
    }
    if body.chars().any(|c| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))) {
        return Err(format!("「{p}」不合法：只能包含字母、数字、_ . -（末尾可以有 *）"));
    }
    Ok(())
}

/// 规则集（`HubConfig.policy`、`Hub::set_policy`、`<home>/policy.json` 与各绑定共用的 JSON 形式）：
/// `{"rules": [{"id", "action": "hide"|"deny", "app", "tool"?, "annotations"?: {"destructiveHint": true, …},
/// "hooks"?: ["call", "wake"]}]}`。未知字段报错；规则按顺序匹配，`deny` 取第一条命中的规则。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PolicyConfig {
    #[serde(default)]
    pub rules: Vec<PolicyRule>,
}

impl PolicyConfig {
    /// 解析并校验 JSON 文本。
    ///
    /// @error 中文说明：JSON 不合法、未知字段、规则不合法、`id` 重复、规则过多。
    pub fn from_json(text: &str) -> Result<Self, String> {
        let config: PolicyConfig = serde_json::from_str(text).map_err(|e| format!("策略规则不是合法的 JSON：{e}"))?;
        config.validate()?;
        Ok(config)
    }

    /// 校验全部规则（`Hub::start`、`Hub::set_policy` 调用）。
    pub fn validate(&self) -> Result<(), String> {
        if self.rules.len() > MAX_POLICY_RULES {
            return Err(format!("策略规则共 {} 条，超过上限 {MAX_POLICY_RULES}", self.rules.len()));
        }
        let mut ids = std::collections::HashSet::new();
        for r in &self.rules {
            r.validate()?;
            if !ids.insert(r.id.as_str()) {
                return Err(format!("规则 id「{}」重复", r.id));
            }
        }
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// 隐藏整个 App 的第一条规则（下标）。
    pub(crate) fn app_hidden(&self, app_id: &str) -> Option<usize> {
        self.rules
            .iter()
            .position(|r| r.action == PolicyAction::Hide && r.matches(app_id, None))
    }

    /// 隐藏该工具的第一条规则（含 App 级 `hide`）。
    pub(crate) fn tool_hidden(&self, app_id: &str, tool: &str, annotations: Option<&ToolAnnotations>) -> Option<usize> {
        self.rules
            .iter()
            .position(|r| r.action == PolicyAction::Hide && r.matches(app_id, Some((tool, annotations))))
    }

    /// 在执行点 `hook` 拒绝的第一条规则。`tool` 为 `None` 时只有 App 级规则匹配。
    pub(crate) fn denied(
        &self,
        hook: PolicyHook,
        app_id: &str,
        tool: Option<(&str, Option<&ToolAnnotations>)>,
    ) -> Option<usize> {
        self.rules.iter().position(|r| r.denies_at(hook) && r.matches(app_id, tool))
    }

    /// 是否有 `hide` 规则（没有时列表不必过滤）。
    pub(crate) fn has_hide(&self) -> bool {
        self.rules.iter().any(|r| r.action == PolicyAction::Hide)
    }

    /// 匹配时是否需要知道工具的注解（有按注解匹配的规则）。
    pub(crate) fn needs_annotations(&self) -> bool {
        self.rules.iter().any(|r| r.annotations.is_some())
    }
}

/// 被 `deny` 规则拒绝时的错误（spec/protocol.md 第 4 节 `POLICY_DENIED`）：只附规则 `id`，不附规则内容。
pub(crate) fn denied_error(rule_id: &str, hook: PolicyHook, app_id: &str, tool: Option<&str>) -> ToolError {
    let target = tool.map_or_else(|| format!("App「{app_id}」"), |t| format!("「{app_id}.{t}」"));
    let what = match hook {
        PolicyHook::Wake => "唤醒",
        _ => "调用",
    };
    ToolError::new(
        ErrorKind::PolicyDenied,
        format!(
            "对{target}的{what}被本机的策略规则「{rule_id}」拒绝，操作未执行。这是用户 / 厂商的设置，重试不会改变结果；需要时请用户调整规则。"
        ),
    )
    .with_details(json!({ "ruleId": rule_id, "hook": hook.as_str(), "appId": app_id, "tool": tool }))
}

/// 一条规则及其命中次数（`/status` 的 `policy.rules`）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicyRuleStatus {
    #[serde(flatten)]
    pub rule: PolicyRule,
    /// 自本规则集生效以来，该规则拒绝或按不存在处理的调用 / 唤醒次数（列表过滤不计）。
    pub hits: u64,
}

/// 最近一次加载规则失败（之前的规则继续生效）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicyLoadError {
    pub message: String,
    pub at_ms: u64,
}

/// 策略状态（`HubStatus.policy`）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicyStatus {
    /// 生效的规则（按顺序）与命中次数。
    pub rules: Vec<PolicyRuleStatus>,
    /// 当前规则集生效的时刻（Unix 毫秒）。
    pub loaded_at_ms: u64,
    /// 最近一次 `set_policy` / `POST /policy` 失败的原因；之后成功加载时清除。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<PolicyLoadError>,
}

/// 运行时状态：生效的规则集、命中计数与最近的加载错误（[`crate::hub::HubShared`] 持有，加锁访问）。
#[derive(Debug, Default)]
pub(crate) struct PolicyState {
    pub config: std::sync::Arc<PolicyConfig>,
    /// 与 `config.rules` 一一对应。
    hits: Vec<u64>,
    loaded_at_ms: u64,
    last_error: Option<PolicyLoadError>,
}

impl PolicyState {
    pub fn new(config: PolicyConfig, now_ms: u64) -> Self {
        let hits = vec![0; config.rules.len()];
        Self { config: std::sync::Arc::new(config), hits, loaded_at_ms: now_ms, last_error: None }
    }

    /// 替换规则集：校验失败时保留之前的规则并记下错误。成功时命中计数清零。
    pub fn replace(&mut self, config: PolicyConfig, now_ms: u64) -> Result<(), String> {
        if let Err(e) = config.validate() {
            self.record_error(&e, now_ms);
            return Err(e);
        }
        *self = Self::new(config, now_ms);
        Ok(())
    }

    /// 记下一次加载失败（规则文本无法解析等），之前的规则继续生效。
    pub fn record_error(&mut self, message: &str, now_ms: u64) {
        self.last_error = Some(PolicyLoadError { message: message.to_owned(), at_ms: now_ms });
    }

    /// 记一次命中（`index` 为规则下标，来自同一个 `config`）。
    pub fn hit(&mut self, config: &std::sync::Arc<PolicyConfig>, index: usize) {
        // 规则集已被替换时不计（下标属于旧规则集）。
        if std::sync::Arc::ptr_eq(config, &self.config)
            && let Some(h) = self.hits.get_mut(index)
        {
            *h += 1;
        }
    }

    pub fn status(&self) -> PolicyStatus {
        PolicyStatus {
            rules: self
                .config
                .rules
                .iter()
                .zip(&self.hits)
                .map(|(rule, hits)| PolicyRuleStatus { rule: rule.clone(), hits: *hits })
                .collect(),
            loaded_at_ms: self.loaded_at_ms,
            last_error: self.last_error.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(v: serde_json::Value) -> PolicyConfig {
        PolicyConfig::from_json(&v.to_string()).expect("valid")
    }

    fn ann(destructive: Option<bool>) -> ToolAnnotations {
        ToolAnnotations { destructive_hint: destructive, ..ToolAnnotations::default() }
    }

    #[test]
    fn empty_policy_allows_everything() {
        let p = PolicyConfig::default();
        assert!(p.is_empty() && !p.has_hide());
        assert_eq!(p.app_hidden("shop"), None);
        assert_eq!(p.tool_hidden("shop", "cart.add", None), None);
        assert_eq!(p.denied(PolicyHook::Call, "shop", Some(("cart.add", None))), None);
        assert_eq!(PolicyConfig::from_json("{}").unwrap(), p);
    }

    #[test]
    fn patterns() {
        assert!(pattern_matches("*", "anything"));
        assert!(pattern_matches("cart.*", "cart.add"));
        assert!(!pattern_matches("cart.*", "cart"));
        assert!(pattern_matches("shop", "shop"));
        assert!(!pattern_matches("shop", "shop2"));
        assert!(pattern_matches("shop*", "shop2"));
    }

    #[test]
    fn hide_app_and_tool_levels() {
        let p = rules(json!({"rules": [
            {"id": "h-app", "action": "hide", "app": "secret*"},
            {"id": "h-tool", "action": "hide", "app": "shop", "tool": "admin.*"},
            {"id": "h-ann", "action": "hide", "app": "*", "annotations": {"destructiveHint": true}},
        ]}));
        assert!(p.has_hide());
        assert_eq!(p.app_hidden("secret-notes"), Some(0));
        assert_eq!(p.app_hidden("shop"), None, "工具级规则不隐藏 App 本身");
        assert_eq!(p.tool_hidden("secret-notes", "x", None), Some(0));
        assert_eq!(p.tool_hidden("shop", "admin.reset", None), Some(1));
        assert_eq!(p.tool_hidden("shop", "cart.add", None), None);
        assert_eq!(p.tool_hidden("shop", "cart.clear", Some(&ann(Some(true)))), Some(2));
        assert_eq!(p.tool_hidden("shop", "cart.clear", Some(&ann(Some(false)))), None);
        assert_eq!(p.tool_hidden("shop", "cart.clear", Some(&ann(None))), None, "未声明的注解不匹配");
        // hide 规则不产生 deny。
        assert_eq!(p.denied(PolicyHook::Call, "shop", Some(("admin.reset", None))), None);
    }

    #[test]
    fn deny_hooks_and_first_match() {
        let p = rules(json!({"rules": [
            {"id": "no-pay", "action": "deny", "app": "shop", "tool": "pay*"},
            {"id": "no-wake", "action": "deny", "app": "music", "hooks": ["wake"]},
            {"id": "both", "action": "deny", "app": "shop", "hooks": ["call", "wake"]},
        ]}));
        assert_eq!(p.denied(PolicyHook::Call, "shop", Some(("pay", None))), Some(0));
        assert_eq!(p.denied(PolicyHook::Call, "shop", Some(("cart.add", None))), Some(2));
        // 缺省只作用于 call。
        assert_eq!(p.denied(PolicyHook::Wake, "shop", Some(("pay", None))), Some(2));
        assert_eq!(p.denied(PolicyHook::Call, "music", Some(("play", None))), None);
        assert_eq!(p.denied(PolicyHook::Wake, "music", Some(("play", None))), Some(1));
        // 不针对工具的唤醒（资源读取）只匹配 App 级规则。
        assert_eq!(p.denied(PolicyHook::Wake, "music", None), Some(1));
        let tool_only = rules(json!({"rules": [{"id": "t", "action": "deny", "app": "a", "tool": "x", "hooks": ["wake"]}]}));
        assert_eq!(tool_only.denied(PolicyHook::Wake, "a", None), None);
    }

    #[test]
    fn validation_errors() {
        let bad = |v: serde_json::Value| PolicyConfig::from_json(&v.to_string()).unwrap_err();
        assert!(bad(json!({"rules": [{"id": "", "action": "hide", "app": "a"}]})).contains("id"));
        assert!(bad(json!({"rules": [{"id": "a b", "action": "hide", "app": "a"}]})).contains("id"));
        assert!(bad(json!({"rules": [{"id": "x", "action": "hide", "app": "a*b"}]})).contains("末尾"));
        assert!(bad(json!({"rules": [{"id": "x", "action": "hide", "app": "a", "hooks": ["call"]}]})).contains("hide"));
        assert!(bad(json!({"rules": [{"id": "x", "action": "deny", "app": "a", "hooks": []}]})).contains("hooks"));
        assert!(bad(json!({"rules": [{"id": "x", "action": "deny", "app": "a", "hooks": ["list"]}]})).contains("hide"));
        assert!(bad(json!({"rules": [{"id": "x", "action": "deny", "app": "a", "hooks": ["handle"]}]})).contains("尚未实现"));
        assert!(bad(json!({"rules": [{"id": "x", "action": "deny", "app": "a", "annotations": {}}]})).contains("annotations"));
        assert!(bad(json!({"rules": [{"id": "x", "action": "deny", "app": "a", "agent": "claude"}]})).contains("JSON"));
        assert!(bad(json!({"rules": [{"id": "x", "action": "block", "app": "a"}]})).contains("JSON"));
        assert!(bad(json!({"rules": [
            {"id": "x", "action": "hide", "app": "a"}, {"id": "x", "action": "deny", "app": "b"}
        ]})).contains("重复"));
        assert!(bad(json!({"version": 1})).contains("JSON"));
        let many: Vec<_> = (0..=MAX_POLICY_RULES).map(|i| json!({"id": format!("r{i}"), "action": "hide", "app": "a"})).collect();
        assert!(bad(json!({"rules": many})).contains("上限"));
    }

    #[test]
    fn denied_error_carries_rule_id_only() {
        let e = denied_error("no-pay", PolicyHook::Call, "shop", Some("pay"));
        assert_eq!(e.kind, ErrorKind::PolicyDenied);
        assert_eq!(e.details, Some(json!({"ruleId": "no-pay", "hook": "call", "appId": "shop", "tool": "pay"})));
        assert!(e.message.contains("no-pay"));
    }

    #[test]
    fn state_keeps_previous_rules_on_error_and_counts_hits() {
        let mut st = PolicyState::new(rules(json!({"rules": [{"id": "a", "action": "hide", "app": "x"}]})), 1);
        let cfg = st.config.clone();
        st.hit(&cfg, 0);
        st.hit(&cfg, 7);
        assert_eq!(st.status().rules[0].hits, 1);
        let bad = PolicyConfig { rules: vec![PolicyRule {
            id: "".into(), action: PolicyAction::Hide, app: "x".into(), tool: None, annotations: None, hooks: None,
        }] };
        assert!(st.replace(bad, 2).is_err());
        let s = st.status();
        assert_eq!((s.rules.len(), s.rules[0].hits, s.loaded_at_ms), (1, 1, 1));
        assert_eq!(s.last_error.as_ref().map(|e| e.at_ms), Some(2));
        st.replace(PolicyConfig::default(), 3).unwrap();
        let s = st.status();
        assert!(s.rules.is_empty() && s.last_error.is_none() && s.loaded_at_ms == 3);
        // 旧规则集的命中不计入新规则集。
        st.hit(&cfg, 0);
        assert!(st.status().rules.is_empty());
        let json = serde_json::to_value(PolicyRuleStatus { rule: cfg.rules[0].clone(), hits: 2 }).unwrap();
        assert_eq!(json, json!({"id": "a", "action": "hide", "app": "x", "hits": 2}));
    }
}
