//! 撤销信息（spec/protocol.md 3.8，第 15 项 X2）：handler 在结果中显式给出逆操作，Hub 只记录与转发（spec/hub-api.md 3.23）。

use super::*;

/// `label` 的最大字符数。
pub const MAX_UNDO_LABEL_CHARS: usize = 200;

/// `arguments` 序列化后的最大字节数。
pub const MAX_UNDO_ARGUMENTS_BYTES: usize = 64 * 1024;

/// 撤销本次调用的逆操作：调用同一 App 的工具 `tool`，参数为 `arguments`。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UndoAction {
    /// 同一 App 的工具局部名（[`is_valid_name`]），可为自身。
    pub tool: String,
    /// 调用逆工具的参数（JSON 对象），缺省 `{}`。
    #[serde(default = "empty_object")]
    pub arguments: Value,
    /// 1..=[`MAX_UNDO_LABEL_CHARS`] 个字符，面向用户：撤销会做什么。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

fn empty_object() -> Value {
    Value::Object(serde_json::Map::new())
}

impl UndoAction {
    /// 只有逆工具名、参数为 `{}` 的撤销信息。
    pub fn new(tool: impl Into<String>) -> Self {
        Self { tool: tool.into(), arguments: empty_object(), label: None }
    }

    /// 从结果中的 `undo` 字段解析并校验（Hub 的宽松解析与 SDK 核心发送前的校验共用此规则）。
    ///
    /// @error 原因文本（面向开发者），见 [`UndoAction::validate`]；另有：不是对象、字段类型不对。
    pub fn parse(value: &Value) -> Result<Self, String> {
        let action: Self = serde_json::from_value(value.clone()).map_err(|e| format!("undo 格式不对：{e}"))?;
        action.validate()?;
        Ok(action)
    }

    /// 格式校验。
    ///
    /// @error `tool` 不是合法局部名、`arguments` 不是对象或序列化后超过 [`MAX_UNDO_ARGUMENTS_BYTES`]、`label` 为空或超长。
    pub fn validate(&self) -> Result<(), String> {
        if !is_valid_name(&self.tool) {
            return Err(format!("undo.tool `{}` 不是合法的工具局部名", self.tool));
        }
        if !self.arguments.is_object() {
            return Err("undo.arguments 须为 JSON 对象".into());
        }
        let bytes = self.arguments.to_string().len();
        if bytes > MAX_UNDO_ARGUMENTS_BYTES {
            return Err(format!("undo.arguments 序列化后 {bytes} 字节，超过上限 {MAX_UNDO_ARGUMENTS_BYTES}"));
        }
        if let Some(label) = &self.label {
            let chars = label.chars().count();
            if label.trim().is_empty() || chars > MAX_UNDO_LABEL_CHARS {
                return Err(format!("undo.label 须为 1..={MAX_UNDO_LABEL_CHARS} 个字符的非空文本（为 {chars} 个）"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parses_minimal_and_full_forms() {
        let min = UndoAction::parse(&json!({"tool": "todo.remove"})).expect("最小形式");
        assert_eq!(min, UndoAction::new("todo.remove"));
        assert_eq!(min.arguments, json!({}));
        let full = UndoAction::parse(&json!({"tool": "t", "arguments": {"id": 3}, "label": "删除刚添加的待办"})).expect("完整形式");
        assert_eq!(full.arguments, json!({"id": 3}));
        assert_eq!(full.label.as_deref(), Some("删除刚添加的待办"));
        // 序列化：label 缺省时省略，arguments 总在
        assert_eq!(serde_json::to_value(&min).unwrap(), json!({"tool": "todo.remove", "arguments": {}}));
    }

    /// 每条拒绝规则一例。
    #[test]
    fn rejects_each_invalid_form() {
        let label_max = "字".repeat(MAX_UNDO_LABEL_CHARS);
        assert!(UndoAction::parse(&json!({"tool": "t", "label": label_max})).is_ok(), "上限边界合法");
        let too_big = "x".repeat(MAX_UNDO_ARGUMENTS_BYTES);
        let cases = [
            (json!("t"), "格式不对"),
            (json!({"arguments": {}}), "格式不对"),
            (json!({"tool": 3}), "格式不对"),
            (json!({"tool": "bad name"}), "不是合法的工具局部名"),
            (json!({"tool": ""}), "不是合法的工具局部名"),
            (json!({"tool": "t", "arguments": [1]}), "须为 JSON 对象"),
            (json!({"tool": "t", "arguments": null}), "须为 JSON 对象"),
            (json!({"tool": "t", "arguments": {"v": too_big}}), "超过上限"),
            (json!({"tool": "t", "label": "  "}), "非空文本"),
            (json!({"tool": "t", "label": "字".repeat(MAX_UNDO_LABEL_CHARS + 1)}), "非空文本"),
            (json!({"tool": "t", "label": 1}), "格式不对"),
        ];
        for (value, reason) in cases {
            let err = UndoAction::parse(&value).expect_err(&value.to_string());
            assert!(err.contains(reason), "{value}: {err}");
        }
    }

    /// 结果中的 `undo` 保留原始 JSON：不合法时不影响结果其余字段的解析（Host 宽松处理的前提）。
    #[test]
    fn invalid_undo_does_not_break_result_parsing() {
        let r: ToolsInvokeResult = serde_json::from_value(json!({"data": {"ok": true}, "status": "partial", "undo": 42})).unwrap();
        assert_eq!(r.data, json!({"ok": true}));
        assert_eq!(r.status, ResultStatus::Partial);
        assert_eq!(r.undo, Some(json!(42)));
        let plain: ToolsInvokeResult = serde_json::from_value(json!({"data": null})).unwrap();
        assert_eq!(plain.undo, None);
        assert!(!serde_json::to_string(&plain).unwrap().contains("undo"));
    }
}
