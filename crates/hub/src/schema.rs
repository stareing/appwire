//! 按工具的 inputSchema 校验调用参数。

use serde_json::Value;

/// 最多报告的错误条数。
#[cfg(feature = "schema-validation")]
const MAX_REPORTED: usize = 5;

/// 校验结果。
#[derive(Debug, PartialEq, Eq)]
pub enum SchemaCheck {
    Valid,
    /// 参数不合法，附带面向模型的说明。
    Invalid(String),
    /// schema 本身无法编译（App 的问题）；Host 跳过校验，交给 SDK 处理。
    BadSchema(String),
    /// 本构建未包含参数校验（feature `schema-validation` 关闭），参数原样交给 App。
    Unchecked,
}

/// 本构建未包含参数校验：一律 [`SchemaCheck::Unchecked`]。
#[cfg(not(feature = "schema-validation"))]
pub fn check(_schema: &Value, _args: &Value) -> SchemaCheck {
    SchemaCheck::Unchecked
}

/// 用 `schema` 校验 `args`。
#[cfg(feature = "schema-validation")]
pub fn check(schema: &Value, args: &Value) -> SchemaCheck {
    let validator = match jsonschema::validator_for(schema) {
        Ok(v) => v,
        Err(e) => return SchemaCheck::BadSchema(e.to_string()),
    };
    let errors: Vec<String> = validator
        .iter_errors(args)
        .take(MAX_REPORTED + 1)
        .map(|e| {
            let path = e.instance_path().to_string();
            if path.is_empty() {
                e.to_string()
            } else {
                format!("{path}: {e}")
            }
        })
        .collect();
    if errors.is_empty() {
        return SchemaCheck::Valid;
    }
    let mut msg = errors
        .iter()
        .take(MAX_REPORTED)
        .cloned()
        .collect::<Vec<_>>()
        .join("；");
    if errors.len() > MAX_REPORTED {
        msg.push_str("；……");
    }
    SchemaCheck::Invalid(msg)
}

#[cfg(all(test, not(feature = "schema-validation")))]
mod unchecked_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn without_feature_everything_is_unchecked() {
        assert_eq!(
            check(&json!({"type": "string"}), &json!(1)),
            SchemaCheck::Unchecked
        );
    }
}

#[cfg(all(test, feature = "schema-validation"))]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema() -> Value {
        json!({
            "type": "object",
            "properties": { "keyword": { "type": "string" }, "limit": { "type": "integer", "minimum": 1 } },
            "required": ["keyword"],
            "additionalProperties": false
        })
    }

    #[test]
    fn valid() {
        assert_eq!(
            check(&schema(), &json!({"keyword": "x", "limit": 3})),
            SchemaCheck::Valid
        );
    }

    #[test]
    fn invalid_reports_path() {
        match check(&schema(), &json!({"keyword": 1, "limit": 0})) {
            SchemaCheck::Invalid(msg) => {
                assert!(msg.contains("/keyword"), "{msg}");
                assert!(msg.contains("/limit"), "{msg}");
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            check(&schema(), &json!({})),
            SchemaCheck::Invalid(_)
        ));
        assert!(matches!(
            check(&schema(), &json!({"keyword": "x", "extra": 1})),
            SchemaCheck::Invalid(_)
        ));
    }

    #[test]
    fn bad_schema() {
        assert!(matches!(
            check(&json!({"type": 12}), &json!({})),
            SchemaCheck::BadSchema(_)
        ));
    }

    #[test]
    fn remote_ref_is_not_fetched() {
        let s =
            json!({"type": "object", "properties": {"a": {"$ref": "http://127.0.0.1:1/x.json"}}});
        assert!(matches!(
            check(&s, &json!({"a": 1})),
            SchemaCheck::BadSchema(_)
        ));
    }
}
