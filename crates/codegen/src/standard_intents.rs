//! 标准意图到系统意图框架的绑定（spec/intents.md 第 3 节，`--standard-intents`）：从模型中挑出声明了 `implements`、
//! 动词在词表中且 `inputSchema` 兼容的工具，交给各原生 target 按自己的映射表输出系统意图版本。
//!
//! 各平台的映射表（系统意图标识、参数转换）只在对应 target 中定义；这里只做与平台无关的筛选与去重，不产生文件。

use app_mcp_protocol::intents::{self, Compatibility, IntentDef, IntentRef};

use crate::schema::{Model, ToolModel, Warning};

/// 一个工具对一个系统意图的绑定。
#[derive(Clone, Copy, Debug)]
pub struct Binding<'m, M> {
    pub tool: &'m ToolModel,
    pub intent: &'static IntentDef,
    /// target 映射表中的条目。
    pub system: M,
}

/// 按 target 的映射表 `map` 与按工具的检查 `resolve` 收集绑定，按工具在模型中的顺序。
///
/// - 词表外的动词、`inputSchema` 不兼容的工具、平台没有对应系统意图的动词：跳过并给出警告（工具的自定义意图照常生成）。
/// - `resolve` 按工具细查（参数能否由系统意图提供等），返回 `None` 表示跳过，警告由它自己写入。
/// - 同一动词版本有多个工具声明：只绑定第一个**通过 `resolve`** 的，其余警告（系统意图在一个 App 内只能有一个处理者）。
///
/// @input `platform` 只用于警告文本，如 `Android`。
/// @invariant 去重在 `resolve` 之后：第一个工具被 target 跳过时，同一动词的下一个工具仍会被尝试。
pub fn bindings<'m, M, B>(
    model: &'m Model,
    platform: &str,
    map: impl Fn(&IntentDef) -> Option<M>,
    mut resolve: impl FnMut(Binding<'m, M>, &mut Vec<Warning>) -> Option<B>,
    warnings: &mut Vec<Warning>,
) -> Vec<B> {
    let mut bound: Vec<(&'static IntentDef, &'m str)> = Vec::new();
    let mut out = Vec::new();
    for tool in &model.tools {
        for item in &tool.info.implements {
            let mut warn = |message: String| {
                warnings.push(Warning { tool: tool.info.name.clone(), path: String::new(), message });
            };
            // 格式已由清单校验保证；这里仍按失败分支处理，不 panic
            let Ok(intent_ref) = IntentRef::parse(item) else {
                warn(format!("implements `{item}` 格式不合法，不生成系统意图"));
                continue;
            };
            let Some(def) = intent_ref.lookup() else {
                warn(format!("`{item}` 不在标准意图词表中，不生成系统意图"));
                continue;
            };
            if let Compatibility::Incompatible(reason) = intents::compatibility(intent_ref, &tool.info.input_schema) {
                warn(format!("与 `{item}` 不兼容（{reason}），不生成系统意图"));
                continue;
            }
            let Some(system) = map(def) else {
                warn(format!("`{item}` 在 {platform} 上没有对应的系统意图，只生成自定义意图"));
                continue;
            };
            if let Some((_, first)) = bound.iter().find(|(d, _)| std::ptr::eq(*d, def)) {
                warn(format!("`{item}` 已由工具 {first} 绑定到系统意图，本工具只生成自定义意图"));
                continue;
            }
            if let Some(b) = resolve(Binding { tool, intent: def, system }, warnings) {
                bound.push((def, &tool.info.name));
                out.push(b);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    fn model(tools: Value) -> Model {
        let manifest = json!({"manifestVersion": 1, "appId": "demo", "name": "Demo", "tools": tools});
        let manifest = serde_json::from_value(manifest).expect("清单");
        crate::schema::build_filtered(&manifest, None, None, |_| true)
    }

    fn tool(name: &str, implements: &[&str], properties: Value) -> Value {
        json!({"name": name, "description": "d", "implements": implements,
               "inputSchema": {"type": "object", "properties": properties}})
    }

    fn send_props() -> Value {
        json!({"to": {"type": "array", "items": {"type": "string"}}, "text": {"type": "string"}})
    }

    /// 只映射 message.send 与 link.open 的假映射表。
    fn map(def: &IntentDef) -> Option<&'static str> {
        match def.verb {
            "message.send" => Some("SEND"),
            "link.open" => Some("VIEW"),
            _ => None,
        }
    }

    fn run(tools: Value) -> (Vec<(String, String, &'static str)>, Vec<String>) {
        let m = model(tools);
        let mut warnings = Vec::new();
        // 名称以 `reject.` 开头的工具由 target 拒绝（模拟按工具的细查）
        let resolve = |b: Binding<'_, &'static str>, w: &mut Vec<Warning>| {
            if b.tool.info.name.starts_with("reject.") {
                w.push(Warning { tool: b.tool.info.name.clone(), path: String::new(), message: "target 拒绝".into() });
                return None;
            }
            Some((b.tool.info.name.clone(), b.intent.id(), b.system))
        };
        let found = bindings(&m, "测试平台", map, resolve, &mut warnings);
        (found, warnings.into_iter().map(|w| w.to_string()).collect())
    }

    #[test]
    fn binds_compatible_mapped_tools() {
        let (found, warnings) = run(json!([
            tool("mail.send", &["message.send@1", "link.open@1"], json!({
                "to": {"type": "array", "items": {"type": "string"}}, "text": {"type": "string"}, "url": {"type": "string"}})),
            tool("plain", &[], json!({})),
        ]));
        assert_eq!(found, vec![
            ("mail.send".into(), "message.send@1".into(), "SEND"),
            ("mail.send".into(), "link.open@1".into(), "VIEW"),
        ]);
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    /// target 拒绝第一个声明者时，同一动词的下一个工具接替绑定。
    #[test]
    fn dedup_after_target_rejects() {
        let (found, warnings) = run(json!([
            tool("reject.first", &["message.send@1"], send_props()),
            tool("b.second", &["message.send@1"], send_props()),
            tool("c.third", &["message.send@1"], send_props()),
        ]));
        assert_eq!(found, vec![("b.second".into(), "message.send@1".into(), "SEND")]);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(warnings[0].contains("reject.first") && warnings[0].contains("target 拒绝"));
        assert!(warnings[1].contains("c.third") && warnings[1].contains("已由工具 b.second 绑定"));
    }

    /// 每条跳过规则一个工具：词表外、不兼容、平台无映射、重复绑定。
    #[test]
    fn skips_with_warnings() {
        let (found, warnings) = run(json!([
            tool("a.unknown", &["x.custom@1"], json!({})),
            tool("b.incompatible", &["message.send@1"], json!({"to": {"type": "string"}})),
            tool("c.unmapped", &["media.play@1"], json!({"query": {"type": "string"}})),
            tool("d.first", &["message.send@1"], send_props()),
            tool("e.second", &["message.send@1"], send_props()),
        ]));
        assert_eq!(found, vec![("d.first".into(), "message.send@1".into(), "SEND")]);
        let expect = [
            ("a.unknown", "不在标准意图词表中"),
            ("b.incompatible", "不兼容"),
            ("c.unmapped", "在 测试平台 上没有对应的系统意图"),
            ("e.second", "已由工具 d.first 绑定"),
        ];
        assert_eq!(warnings.len(), expect.len(), "{warnings:?}");
        for ((tool, text), warning) in expect.iter().zip(&warnings) {
            assert!(warning.contains(tool) && warning.contains(text), "{warning}");
        }
    }
}
