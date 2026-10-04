//! 机主默认表（spec/intents.md 第 4 节）：意图 → 工具全名。默认只是提示：Hub 在 `apps.intents` 中把它排在最前并标
//! `default: true`，不据此路由。

use std::collections::BTreeMap;
use std::sync::Arc;

use app_mcp_protocol::intents::parse_verb_query;
use app_mcp_protocol::is_valid_full_tool_name;
use serde::{Deserialize, Serialize};

/// 默认表的最多条数（B-07：限制文件 / 请求体与每次 `apps.intents` 的查找成本）。
///
/// @why 256：远多于词表动词数与 App 自定义动词的现实数量。
pub const MAX_INTENT_DEFAULTS: usize = 256;

/// `<home>/intents.json` 与 `POST /intents` 的请求体：`{"defaults": {"message.send": "mail.compose.send"}}`。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntentsConfig {
    /// 键：动词（`message.send`，对该动词所有版本生效）或带主版本（`message.send@1`，优先于不带版本的键）；值：工具全名。
    #[serde(default)]
    pub defaults: BTreeMap<String, String>,
}

impl IntentsConfig {
    /// 解析并校验。
    ///
    /// @error JSON 不合法或含未知字段、[`validate_defaults`] 的错误（中文说明）。
    pub fn from_json(text: &str) -> Result<Self, String> {
        let config: Self = serde_json::from_str(text).map_err(|e| format!("不是合法的意图默认表 JSON：{e}"))?;
        validate_defaults(&config.defaults)?;
        Ok(config)
    }
}

/// 校验默认表：键为合法动词（可带 `@<主版本>`），值为合法工具全名 `<appId>.<局部名>`，至多 [`MAX_INTENT_DEFAULTS`] 条。
///
/// @error 全部问题，以「；」连接。
pub fn validate_defaults(defaults: &BTreeMap<String, String>) -> Result<(), String> {
    let mut problems: Vec<String> = Vec::new();
    if defaults.len() > MAX_INTENT_DEFAULTS {
        problems.push(format!("默认表最多 {MAX_INTENT_DEFAULTS} 条（实际 {} 条）", defaults.len()));
    }
    for (intent, tool) in defaults {
        if parse_verb_query(intent).is_err() {
            problems.push(format!("键「{intent}」不是合法的意图（应为 <域>.<动作> 或 <域>.<动作>@<主版本>，如 message.send）"));
        }
        if !is_valid_full_tool_name(tool) {
            problems.push(format!("「{intent}」的值「{tool}」不是合法的工具全名（应为 <appId>.<工具名>）"));
        }
    }
    if problems.is_empty() { Ok(()) } else { Err(problems.join("；")) }
}

/// 某个意图版本的机主默认工具：先找 `<动词>@<版本>`，再找 `<动词>`。
pub(crate) fn default_for<'a>(defaults: &'a BTreeMap<String, String>, verb: &str, version: u32) -> Option<&'a str> {
    defaults.get(&format!("{verb}@{version}")).or_else(|| defaults.get(verb)).map(String::as_str)
}

/// `/status` 的 `intents`：生效的默认表与最近一次替换失败的原因。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IntentsStatus {
    pub defaults: BTreeMap<String, String>,
    /// 最近一次 `set_intent_defaults` / `POST /intents` 失败的原因（之前的默认表继续生效）；之后成功时清除。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

/// 运行时状态（[`crate::hub::HubShared`] 持有，加锁访问）。
#[derive(Debug, Default)]
pub(crate) struct IntentsState {
    pub defaults: Arc<BTreeMap<String, String>>,
    pub last_error: Option<String>,
}

impl IntentsState {
    /// 由配置构造；配置不合法时为空表并记下错误（嵌入式厂商经 `status().intents.last_error` 可见）。
    pub fn new(defaults: &BTreeMap<String, String>) -> Self {
        let mut state = Self::default();
        if let Err(e) = state.replace(defaults.clone()) {
            tracing::error!(error = %e, "HubConfig.intent_defaults 不合法，按空表启动");
        }
        state
    }

    /// 替换默认表；不合法时保留之前的表并记下错误。
    pub fn replace(&mut self, defaults: BTreeMap<String, String>) -> Result<(), String> {
        match validate_defaults(&defaults) {
            Ok(()) => {
                self.defaults = Arc::new(defaults);
                self.last_error = None;
                Ok(())
            }
            Err(e) => {
                self.last_error = Some(e.clone());
                Err(e)
            }
        }
    }

    pub fn status(&self) -> IntentsStatus {
        IntentsStatus { defaults: (*self.defaults).clone(), last_error: self.last_error.clone() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()
    }

    #[test]
    fn validate_rules() {
        assert_eq!(validate_defaults(&map(&[])), Ok(()));
        assert_eq!(
            validate_defaults(&map(&[("message.send", "mail.compose.send"), ("media.play@1", "music.play"), ("x.y@2", "a.b")])),
            Ok(())
        );
        let bad_key = ["message", "message.send@0", "message.send@", "a.b.c", "Message send"];
        for k in bad_key {
            let e = validate_defaults(&map(&[(k, "mail.send")])).unwrap_err();
            assert!(e.contains(k), "{k}: {e}");
        }
        for v in ["mail", "Mail.send", "mail.", ".send", "mail.se nd"] {
            let e = validate_defaults(&map(&[("message.send", v)])).unwrap_err();
            assert!(e.contains("工具全名"), "{v}: {e}");
        }
        let both = validate_defaults(&map(&[("bad", "worse")])).unwrap_err();
        assert_eq!(both.matches('；').count(), 1, "两个问题都列出：{both}");
        let many: BTreeMap<String, String> =
            (0..=MAX_INTENT_DEFAULTS).map(|i| (format!("d.v{i}"), "a.b".to_owned())).collect();
        assert!(validate_defaults(&many).unwrap_err().contains("最多"));
        let max: BTreeMap<String, String> = many.into_iter().take(MAX_INTENT_DEFAULTS).collect();
        assert_eq!(validate_defaults(&max), Ok(()));
    }

    #[test]
    fn from_json_rejects_unknown_fields_and_bad_values() {
        let c = IntentsConfig::from_json(r#"{"defaults": {"message.send": "mail.send"}}"#).unwrap();
        assert_eq!(c.defaults, map(&[("message.send", "mail.send")]));
        assert_eq!(IntentsConfig::from_json("{}").unwrap(), IntentsConfig::default());
        assert!(IntentsConfig::from_json(r#"{"default": {}}"#).is_err());
        assert!(IntentsConfig::from_json(r#"{"defaults": {"message.send": 3}}"#).is_err());
        assert!(IntentsConfig::from_json(r#"{"defaults": {"message.send": "mail"}}"#).is_err());
        assert!(IntentsConfig::from_json("[").is_err());
    }

    #[test]
    fn versioned_key_wins_over_plain() {
        let d = map(&[("message.send", "a.plain"), ("message.send@2", "a.v2")]);
        assert_eq!(default_for(&d, "message.send", 1), Some("a.plain"));
        assert_eq!(default_for(&d, "message.send", 2), Some("a.v2"));
        assert_eq!(default_for(&d, "media.play", 1), None);
    }

    #[test]
    fn state_keeps_previous_table_on_error() {
        let mut st = IntentsState::new(&map(&[("message.send", "a.b")]));
        assert_eq!(st.status().last_error, None);
        assert!(st.replace(map(&[("bad", "a.b")])).is_err());
        assert_eq!(*st.defaults, map(&[("message.send", "a.b")]));
        assert!(st.status().last_error.is_some());
        st.replace(map(&[])).unwrap();
        assert_eq!(st.status(), IntentsStatus::default());
        let invalid = IntentsState::new(&map(&[("bad", "a.b")]));
        assert!(invalid.defaults.is_empty() && invalid.last_error.is_some());
    }
}
