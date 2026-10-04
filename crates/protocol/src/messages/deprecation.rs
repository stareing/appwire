//! 工具弃用声明（spec/protocol.md 3.7，第 16 项 O4）：弃用的工具照常列出与调用，Hub 只呈现（spec/hub-api.md 3.21）。

use super::*;

/// `message` 的最大字符数。
pub const MAX_DEPRECATION_MESSAGE_CHARS: usize = 500;

/// 工具弃用声明。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Deprecation {
    /// 1..=[`MAX_DEPRECATION_MESSAGE_CHARS`] 个字符，面向模型：为什么弃用、该怎么做。
    pub message: String,
    /// 替代工具：同一 App 中的局部名（[`is_valid_name`]）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replacement: Option<String>,
    /// 计划移除的日期（RFC 3339 full-date，`YYYY-MM-DD`），只作提示。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<String>,
}

impl Deprecation {
    /// 格式校验。`tool` 为声明所在工具的局部名（`replacement` 不得指向自身）。
    ///
    /// @error 原因文本（面向开发者）：`message` 为空或超长、`replacement` 不合法或指向自身、`until` 不是合法日期。
    pub fn validate(&self, tool: &str) -> Result<(), String> {
        let chars = self.message.chars().count();
        if self.message.trim().is_empty() || chars > MAX_DEPRECATION_MESSAGE_CHARS {
            return Err(format!("deprecated.message 须为 1..={MAX_DEPRECATION_MESSAGE_CHARS} 个字符的非空文本（为 {chars} 个）"));
        }
        if let Some(r) = &self.replacement {
            if !is_valid_name(r) {
                return Err(format!("deprecated.replacement `{r}` 不是合法的工具局部名"));
            }
            if r == tool {
                return Err(format!("deprecated.replacement 不能指向工具自身 `{tool}`"));
            }
        }
        if let Some(d) = &self.until
            && !is_full_date(d)
        {
            return Err(format!("deprecated.until `{d}` 不是 RFC 3339 日期（YYYY-MM-DD）"));
        }
        Ok(())
    }
}

/// `inputSchema` 顶层 `required` 中、对应属性标了 JSON Schema `deprecated: true` 的参数名（spec/protocol.md 3.7：矛盾声明，
/// 清单校验与 SDK 注册给出警告）。按 `required` 中的顺序，不去重；schema 结构不合预期时视为无。
pub fn deprecated_required_params(input_schema: &Value) -> Vec<&str> {
    let properties = input_schema.get("properties");
    let is_deprecated = |name: &str| properties.and_then(|p| p.get(name)).and_then(|p| p.get("deprecated")) == Some(&Value::Bool(true));
    input_schema
        .get("required")
        .and_then(Value::as_array)
        .map(|req| req.iter().filter_map(Value::as_str).filter(|n| is_deprecated(n)).collect())
        .unwrap_or_default()
}

/// RFC 3339 full-date：`YYYY-MM-DD`，月份与当月天数（含闰年）合法。
fn is_full_date(text: &str) -> bool {
    let b = text.as_bytes();
    let digits = |r: std::ops::Range<usize>| b[r.clone()].iter().all(u8::is_ascii_digit).then(|| text[r].parse::<u32>().ok()).flatten();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    let (Some(y), Some(m), Some(d)) = (digits(0..4), digits(5..7), digits(8..10)) else {
        return false;
    };
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let days = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1..=days).contains(&d)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn dep(message: &str, replacement: Option<&str>, until: Option<&str>) -> Deprecation {
        Deprecation { message: message.into(), replacement: replacement.map(Into::into), until: until.map(Into::into) }
    }

    #[test]
    fn serde_omits_absent_fields() {
        let d: Deprecation = serde_json::from_value(json!({"message": "改用 v2"})).unwrap();
        assert_eq!(d, dep("改用 v2", None, None));
        assert_eq!(serde_json::to_value(&d).unwrap(), json!({"message": "改用 v2"}));
        let full = dep("m", Some("orders.list2"), Some("2027-06-30"));
        assert_eq!(serde_json::to_value(&full).unwrap(), json!({"message": "m", "replacement": "orders.list2", "until": "2027-06-30"}));
    }

    /// 只认顶层 `required` 中属性的 `deprecated: true`；非布尔、未列为必填、schema 结构异常时为空。
    #[test]
    fn deprecated_required_params_rules() {
        let schema = json!({
            "type": "object",
            "properties": {
                "a": {"type": "string", "deprecated": true},
                "b": {"type": "string", "deprecated": false},
                "c": {"type": "string", "deprecated": "true"},
                "d": {"type": "string", "deprecated": true},
                "e": {"type": "string"}
            },
            "required": ["a", "b", "c", "e", "missing"]
        });
        assert_eq!(deprecated_required_params(&schema), vec!["a"]);
        assert!(deprecated_required_params(&json!({"type": "object", "properties": {"a": {"deprecated": true}}})).is_empty());
        assert!(deprecated_required_params(&json!({"type": "object", "required": "a"})).is_empty());
        assert!(deprecated_required_params(&json!(true)).is_empty());
    }

    /// 每条规则一个用例（T-09）。
    #[test]
    fn validate_rules() {
        assert!(dep("m", Some("b"), Some("2028-02-29")).validate("a").is_ok());
        assert!(dep(&"字".repeat(MAX_DEPRECATION_MESSAGE_CHARS), None, None).validate("a").is_ok());
        let bad = [
            (dep("", None, None), "message"),
            (dep("  ", None, None), "message"),
            (dep(&"字".repeat(MAX_DEPRECATION_MESSAGE_CHARS + 1), None, None), "message"),
            (dep("m", Some("bad name"), None), "replacement"),
            (dep("m", Some("a"), None), "自身"),
            (dep("m", None, Some("2027-6-30")), "until"),
            (dep("m", None, Some("2027-13-01")), "until"),
            (dep("m", None, Some("2027-02-29")), "until"),
            (dep("m", None, Some("2027-04-31")), "until"),
            (dep("m", None, Some("2027-06-30T00:00:00Z")), "until"),
            (dep("m", None, Some("２０２７-06-30")), "until"),
        ];
        for (d, word) in bad {
            let err = d.validate("a").expect_err(&format!("{d:?}"));
            assert!(err.contains(word), "{err}");
        }
    }
}
