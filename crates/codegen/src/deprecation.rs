//! 弃用声明（spec/protocol.md 3.7，第 16 项 O4）→ 各 target 共用的说明文本。
//!
//! 工具级弃用来自 `ToolInfo.deprecated`，参数级弃用来自属性 schema 的 `deprecated: true`（[`crate::schema::Field::deprecated`]）。
//! 文本只在这里拼一次；各语言的标注写法（`@deprecated`、`[Obsolete]`、`@available`、`@Deprecated` 等）与转义由各 target 负责。

use app_mcp_protocol::Deprecation;

use crate::schema::{Field, ToolModel};

/// 参数级弃用的标注消息（JSON Schema 的 `deprecated` 没有说明文字）。
pub const FIELD_MESSAGE: &str = "参数已弃用（inputSchema 中 deprecated: true）";

/// 工具级弃用的完整消息：`message`，再追加替代工具与计划移除日期（如有）。
///
/// @output 可能含换行（`message` 原样保留）；放进字符串字面量或注释前由调用方按目标语言转义。
pub fn tool_message(dep: &Deprecation) -> String {
    let mut extra = Vec::new();
    if let Some(r) = &dep.replacement {
        extra.push(format!("改用工具 {r}"));
    }
    if let Some(d) = &dep.until {
        extra.push(format!("计划于 {d} 移除"));
    }
    let message = dep.message.trim_end();
    if extra.is_empty() {
        message.to_string()
    } else {
        format!("{message}（{}）", extra.join("；"))
    }
}

/// 工具的弃用消息；未弃用时为 `None`。
pub fn tool_deprecation(tool: &ToolModel) -> Option<String> {
    tool.deprecation().map(tool_message)
}

/// 工具文档注释追加的行（`已弃用：…`，多行消息拆成多行）；未弃用时为空。
pub fn tool_doc_lines(tool: &ToolModel) -> Vec<String> {
    tool_deprecation(tool).map(|m| prefixed_lines("已弃用：", &m)).unwrap_or_default()
}

/// 字段文档注释追加的行；未弃用时为空。
pub fn field_doc_lines(field: &Field) -> Vec<String> {
    if field.deprecated { vec![FIELD_MESSAGE.to_string()] } else { Vec::new() }
}

/// 第一行加前缀，其余行原样（去掉行尾空白），供注释输出。
pub fn prefixed_lines(prefix: &str, text: &str) -> Vec<String> {
    text.lines()
        .enumerate()
        .map(|(i, l)| if i == 0 { format!("{prefix}{}", l.trim_end()) } else { l.trim_end().to_string() })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dep(message: &str, replacement: Option<&str>, until: Option<&str>) -> Deprecation {
        Deprecation { message: message.into(), replacement: replacement.map(Into::into), until: until.map(Into::into) }
    }

    /// 每种可选字段组合一个用例（T-09）。
    #[test]
    fn tool_message_appends_replacement_and_until() {
        let cases = [
            (dep("旧接口", None, None), "旧接口"),
            (dep("旧接口", Some("orders.list2"), None), "旧接口（改用工具 orders.list2）"),
            (dep("旧接口", None, Some("2027-06-30")), "旧接口（计划于 2027-06-30 移除）"),
            (dep("旧接口\n", Some("b"), Some("2027-06-30")), "旧接口（改用工具 b；计划于 2027-06-30 移除）"),
        ];
        for (d, want) in cases {
            assert_eq!(tool_message(&d), want);
        }
    }

    #[test]
    fn prefixed_lines_keeps_line_breaks() {
        assert_eq!(prefixed_lines("已弃用：", "a \nb"), ["已弃用：a", "b"]);
        assert!(prefixed_lines("x", "").is_empty());
    }
}
