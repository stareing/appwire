//! 缓存键与参数规范化（spec/protocol.md 3.6「相同请求」）。

use app_mcp_protocol::CacheScope;
use serde_json::Value;

/// 缓存键：（范围, appId, 工具局部名 + 规范化参数 | 资源名）。
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct CacheKey {
    /// 范围：`Some(记账主体)` = private（`agent:<名>` / `local` / `api`）；`None` = shared（全体调用方共用）。
    pub scope: Option<String>,
    pub app_id: String,
    pub target: CacheTarget,
}

/// 缓存的对象。
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum CacheTarget {
    /// 工具局部名与规范化后的参数（[`canonical_json`]）。
    Tool { name: String, arguments: String },
    /// 资源名（局部名）。
    Resource { name: String },
}

impl CacheKey {
    /// 工具调用的键。
    ///
    /// @input subject 调用方的记账主体（[`crate::task::CallerKey::usage_subject`]）；`scope` 为 shared 时不使用。
    pub(crate) fn tool(scope: CacheScope, subject: &str, app_id: &str, tool: &str, arguments: &Value) -> Self {
        Self {
            scope: scope_key(scope, subject),
            app_id: app_id.to_owned(),
            target: CacheTarget::Tool { name: tool.to_owned(), arguments: canonical_json(arguments) },
        }
    }

    /// 资源读取的键。
    pub(crate) fn resource(scope: CacheScope, subject: &str, app_id: &str, name: &str) -> Self {
        Self { scope: scope_key(scope, subject), app_id: app_id.to_owned(), target: CacheTarget::Resource { name: name.to_owned() } }
    }

    /// 键占用的字节数（计入条目大小）。
    pub(crate) fn bytes(&self) -> usize {
        let target = match &self.target {
            CacheTarget::Tool { name, arguments } => name.len() + arguments.len(),
            CacheTarget::Resource { name } => name.len(),
        };
        self.scope.as_ref().map_or(0, String::len) + self.app_id.len() + target
    }

    /// 是否为资源 `name` 的条目。
    pub(crate) fn is_resource(&self, name: &str) -> bool {
        matches!(&self.target, CacheTarget::Resource { name: n } if n == name)
    }
}

fn scope_key(scope: CacheScope, subject: &str) -> Option<String> {
    match scope {
        CacheScope::Private => Some(subject.to_owned()),
        CacheScope::Shared => None,
    }
}

/// 规范化 JSON：对象键按字节序排序后的紧凑文本（不依赖 serde_json 是否启用 `preserve_order`）。
pub(crate) fn canonical_json(v: &Value) -> String {
    let mut out = String::new();
    write_canonical(v, &mut out);
    out
}

fn write_canonical(v: &Value, out: &mut String) {
    match v {
        Value::Object(map) => {
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            // @invariant `String` 的 Ord 即 UTF-8 字节序。
            entries.sort_unstable_by(|a, b| a.0.cmp(b.0));
            out.push('{');
            for (i, (k, v)) in entries.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                push_string(k, out);
                out.push(':');
                write_canonical(v, out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, v) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(v, out);
            }
            out.push(']');
        }
        Value::String(s) => push_string(s, out),
        other => out.push_str(&other.to_string()),
    }
}

fn push_string(s: &str, out: &mut String) {
    // @invariant 字符串的序列化不会失败。
    out.push_str(&serde_json::to_string(s).unwrap_or_default());
}
