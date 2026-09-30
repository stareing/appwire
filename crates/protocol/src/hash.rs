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
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
            out.push('{');
            for (i, (k, v)) in entries.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
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

fn write_string(s: &str, out: &mut String) {
    out.push_str(&Value::String(s.to_owned()).to_string());
}

fn to_value<T: serde::Serialize>(v: &T) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

/// 计算 `toolsHash`。输入顺序无关（内部按名称排序）。
pub fn tools_hash(tools: &ToolsSyncParams, resources: &ResourcesSyncParams) -> String {
    let mut t: Vec<&ToolInfo> = tools.tools.iter().collect();
    t.sort_by(|a, b| a.name.as_bytes().cmp(b.name.as_bytes()));
    let mut r: Vec<&ResourceInfo> = resources.resources.iter().collect();
    r.sort_by(|a, b| a.name.as_bytes().cmp(b.name.as_bytes()));
    let doc = serde_json::json!({ "resources": to_value(&r), "tools": to_value(&t) });
    let digest = Sha256::digest(canonical_json(&doc).as_bytes());
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Activation, Risk};
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
                },
                ToolInfo {
                    name: "cart.checkout".into(),
                    description: "结算".into(),
                    input_schema: json!({"type": "object"}),
                    risk: Risk::Payment,
                    activation: Some(Activation::Foreground),
                    title: Some("Checkout".into()),
                },
            ],
        }
    }

    fn vector_resources() -> ResourcesSyncParams {
        ResourcesSyncParams {
            resources: vec![ResourceInfo { name: "cart.state".into(), description: "购物车".into(), mime_type: None }],
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
}
