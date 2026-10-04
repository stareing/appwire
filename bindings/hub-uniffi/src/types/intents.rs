//! 标准意图（spec/intents.md 第 4 节）：机主默认表的状态记录。

use std::collections::HashMap;

use app_mcp_hub as hub;

/// `AppMcpHub::intents()` 与 `HubStatus.intents`：生效的机主默认表与最近一次替换失败的原因。
#[derive(Clone, Debug, Default, PartialEq, Eq, uniffi::Record)]
pub struct IntentsStatus {
    /// 动词（或 `动词@主版本`）→ 工具全名。
    pub defaults: HashMap<String, String>,
    /// 最近一次 `set_intent_defaults` 失败（或 `HubConfig.intent_defaults` 不合法）的原因；之后成功时清除。
    #[uniffi(default = None)]
    pub last_error: Option<String>,
}

impl From<hub::IntentsStatus> for IntentsStatus {
    fn from(s: hub::IntentsStatus) -> Self {
        IntentsStatus { defaults: s.defaults.into_iter().collect(), last_error: s.last_error }
    }
}
