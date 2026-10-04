//! 工具摘要 `toolsHash`（spec/lifecycle.md 第 6 节）。
//!
//! `toolsHash = sha256(规范化 JSON({"resources": [...], "tools": [...]})) 的前 16 个十六进制字符`，
//! 其中 `tools` / `resources` 分别是 `tools/sync` 与 `resources/sync` 参数中的数组，按 `name` 排序。
//! 规范化：对象键按 UTF-8 字节序排序、无空白；字符串按 JSON 标准转义（非 ASCII 字符原样输出）；
//! 数字按 serde_json 的格式输出。所有语言都通过核心库计算，结果一致。

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{ResourceInfo, ResourcesSyncParams, ToolInfo, ToolsSyncParams};

/// 规范化 JSON：对象键排序、无空白。与 serde_json 是否启用 `preserve_order` 无关。
pub fn canonical_json(value: &Value) -> String {
    let mut out = String::new();
    write_canonical(value, &mut out);
    out
}

fn write_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let entries: Vec<(&String, &Value)> = map.iter().collect();
            out.push('{');
            for (i, &at) in sorted_order(entries.iter().map(|(k, _)| k.as_str())).iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                let (k, v) = entries[at];
                write_string(k, out);
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
        Value::String(s) => write_string(s, out),
        // null / bool / number 的 serde_json 输出本身就是紧凑且确定的。
        other => out.push_str(&other.to_string()),
    }
}

/// 按名称的 UTF-8 字节序排列后的下标；同名保持输入顺序（结果与稳定排序相同）。
///
/// @why 键与下标组成的元组互不相等，`sort_unstable` 即可得到确定结果；
///      三处排序共用这一个单态化，减小 WASM 体积。
fn sorted_order<'a>(names: impl Iterator<Item = &'a str>) -> Vec<usize> {
    let mut keys: Vec<(&[u8], usize)> = names.enumerate().map(|(i, n)| (n.as_bytes(), i)).collect();
    keys.sort_unstable();
    keys.into_iter().map(|(_, i)| i).collect()
}

fn write_string(s: &str, out: &mut String) {
    out.push_str(&Value::String(s.to_owned()).to_string());
}

fn to_value<T: serde::Serialize>(v: &T) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

/// 计算 `toolsHash`。输入顺序无关（内部按名称排序）。
pub fn tools_hash(tools: &ToolsSyncParams, resources: &ResourcesSyncParams) -> String {
    let t: Vec<&ToolInfo> =
        sorted_order(tools.tools.iter().map(|x| x.name.as_str())).into_iter().map(|i| &tools.tools[i]).collect();
    let r: Vec<&ResourceInfo> = sorted_order(resources.resources.iter().map(|x| x.name.as_str()))
        .into_iter()
        .map(|i| &resources.resources[i])
        .collect();
    let doc = serde_json::json!({ "resources": to_value(&r), "tools": to_value(&t) });
    let digest = Sha256::digest(canonical_json(&doc).as_bytes());
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Activation, Risk, ToolSurface};
    use serde_json::json;

    /// 固定测试向量：各语言 / Host 的一致性测试使用同一组输入与期望值。
    fn vector_tools() -> ToolsSyncParams {
        ToolsSyncParams {
            tools: vec![
                ToolInfo {
                    name: "todo.add".into(),
                    description: "添加待办".into(),
                    input_schema: json!({"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]}),
                    risk: Risk::Write,
                    activation: None,
                    title: None,
                    annotations: None,
                    output_schema: None,
                    surface: ToolSurface::App,
                    page: None,
                    background_tool: None,
                    implements: Vec::new(),
                },
                ToolInfo {
                    name: "cart.checkout".into(),
                    description: "结算".into(),
                    input_schema: json!({"type": "object"}),
                    risk: Risk::Payment,
                    activation: Some(Activation::Foreground),
                    title: Some("Checkout".into()),
                    annotations: None,
                    output_schema: None,
                    surface: ToolSurface::App,
                    page: None,
                    background_tool: None,
                    implements: Vec::new(),
                },
            ],
        }
    }

    fn vector_resources() -> ResourcesSyncParams {
        ResourcesSyncParams {
            resources: vec![ResourceInfo { name: "cart.state".into(), description: "购物车".into(), mime_type: None, realtime: false, annotations: None }],
        }
    }

    #[test]
    fn canonical_form() {
        let v = json!({"b": [1, {"z": null, "a": true}], "a": "x\"\n中"});
        assert_eq!(canonical_json(&v), r#"{"a":"x\"\n中","b":[1,{"a":true,"z":null}]}"#);
    }

    #[test]
    fn fixed_vectors() {
        let empty = tools_hash(&ToolsSyncParams::default(), &ResourcesSyncParams::default());
        // sha256('{"resources":[],"tools":[]}')
        assert_eq!(empty, "69c61b185225ee82");
        let h = tools_hash(&vector_tools(), &vector_resources());
        assert_eq!(h.len(), 16);
        assert_eq!(h, "ba703035ddca2f91");
    }

    #[test]
    fn order_independent_and_sensitive() {
        let mut t = vector_tools();
        let h = tools_hash(&t, &vector_resources());
        t.tools.reverse();
        assert_eq!(tools_hash(&t, &vector_resources()), h);
        t.tools[0].description.push('!');
        assert_ne!(tools_hash(&t, &vector_resources()), h);
        assert_ne!(tools_hash(&vector_tools(), &ResourcesSyncParams::default()), h);
    }

    /// `realtime` 只在为 true 时序列化：未声明的资源摘要不变，声明后摘要变化（spec/lifecycle.md 第 13 节 B3）。
    #[test]
    fn realtime_flag_serialized_only_when_true() {
        let plain = vector_resources();
        assert!(!serde_json::to_string(&plain).unwrap().contains("realtime"));
        let mut rt = plain.clone();
        rt.resources[0].realtime = true;
        assert!(serde_json::to_string(&rt).unwrap().contains(r#""realtime":true"#));
        assert_ne!(tools_hash(&vector_tools(), &rt), tools_hash(&vector_tools(), &plain));
        let parsed: ResourceInfo = serde_json::from_value(json!({"name": "a", "description": "d"})).unwrap();
        assert!(!parsed.realtime);
        let parsed: ResourceInfo = serde_json::from_value(json!({"name": "a", "description": "d", "realtime": true})).unwrap();
        assert!(parsed.realtime);
    }

    /// `implements` 只在非空时序列化：未声明的工具摘要不变（固定向量不变），声明后摘要变化（spec/intents.md 第 1 节）。
    #[test]
    fn implements_serialized_only_when_non_empty() {
        let plain = vector_tools();
        assert!(!serde_json::to_string(&plain).unwrap().contains("implements"));
        let mut declared = plain.clone();
        declared.tools[0].implements = vec!["message.send@1".into()];
        assert!(serde_json::to_string(&declared).unwrap().contains(r#""implements":["message.send@1"]"#));
        assert_ne!(tools_hash(&declared, &vector_resources()), tools_hash(&plain, &vector_resources()));
        let parsed: ToolInfo =
            serde_json::from_value(json!({"name": "a", "description": "d", "inputSchema": {"type": "object"}})).unwrap();
        assert!(parsed.implements.is_empty());
        let parsed: ToolInfo = serde_json::from_value(
            json!({"name": "a", "description": "d", "inputSchema": {"type": "object"}, "implements": ["link.open@1"]}),
        )
        .unwrap();
        assert_eq!(parsed.implements, ["link.open@1"]);
    }

    #[test]
    fn sorted_order_is_byte_order_and_stable_for_equal_names() {
        assert_eq!(sorted_order(["b", "a", "b", "é", "Z"].into_iter()), [4, 1, 0, 2, 3]);
        assert!(sorted_order(std::iter::empty()).is_empty());
    }
}
