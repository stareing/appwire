//! 兼容规则表逐格测试（spec/manifest.md 第 6 节；T-09：每个破坏性 / 可能破坏 / 兼容项至少一个用例）。

use serde_json::{Value, json};

use super::*;

fn tool(name: &str, input: Value) -> ToolInfo {
    serde_json::from_value(json!({"name": name, "description": "d", "inputSchema": input})).expect("ToolInfo")
}

fn with(mut base: ToolInfo, patch: Value) -> ToolInfo {
    let mut v = serde_json::to_value(&base).expect("序列化");
    for (k, val) in patch.as_object().expect("patch 为对象") {
        v[k] = val.clone();
    }
    base = serde_json::from_value(v).expect("ToolInfo");
    base
}

fn obj(props: Value, required: &[&str]) -> Value {
    json!({"type": "object", "properties": props, "required": required})
}

/// 比较两份 inputSchema。
fn input(old: Value, new: Value) -> Vec<SchemaChange> {
    compare_tool(&tool("t", old), &tool("t", new))
}

/// 比较两份 outputSchema（inputSchema 相同）。
fn output(old: Value, new: Value) -> Vec<SchemaChange> {
    let base = tool("t", json!({"type": "object"}));
    compare_tool(&with(base.clone(), json!({"outputSchema": old})), &with(base, json!({"outputSchema": new})))
}

/// 断言恰好一条变化，级别与路径如期。
#[track_caller]
fn one(changes: &[SchemaChange], level: ChangeLevel, path: &str) {
    assert_eq!(changes.len(), 1, "{changes:#?}");
    assert_eq!((changes[0].level, changes[0].path.as_str()), (level, path), "{changes:#?}");
}

#[track_caller]
fn none(changes: &[SchemaChange]) {
    assert!(changes.is_empty(), "{changes:#?}");
}

use ChangeLevel::{Breaking, Warning};

// ---------------------------------------------------------------------------
// inputSchema：破坏性
// ---------------------------------------------------------------------------

#[test]
fn input_new_required_is_breaking() {
    let old = obj(json!({"to": {"type": "string"}}), &[]);
    let new = obj(json!({"to": {"type": "string"}}), &["to"]);
    one(&input(old, new), Breaking, "/inputSchema/properties/to");
    // 新增的属性同时必填：只记一条
    let old = obj(json!({}), &[]);
    let new = obj(json!({"to": {"type": "string"}}), &["to"]);
    one(&input(old, new), Breaking, "/inputSchema/properties/to");
}

#[test]
fn input_removed_property_is_breaking() {
    let old = obj(json!({"to": {"type": "string"}, "cc": {"type": "string"}}), &[]);
    let new = obj(json!({"to": {"type": "string"}}), &[]);
    one(&input(old, new), Breaking, "/inputSchema/properties/cc");
}

#[test]
fn input_type_narrowing_is_breaking() {
    let p = |t: Value| obj(json!({"n": {"type": t}}), &[]);
    one(&input(p(json!(["string", "number"])), p(json!("string"))), Breaking, "/inputSchema/properties/n/type");
    one(&input(p(json!("number")), p(json!("integer"))), Breaking, "/inputSchema/properties/n/type");
    // 缺省视为任意：加上 type 即收窄
    let any = obj(json!({"n": {}}), &[]);
    one(&input(any, p(json!("string"))), Breaking, "/inputSchema/properties/n/type");
}

#[test]
fn input_type_widening_is_compatible() {
    let p = |t: Value| obj(json!({"n": {"type": t}}), &[]);
    none(&input(p(json!("integer")), p(json!("number"))));
    none(&input(p(json!("string")), p(json!(["string", "null"]))));
    none(&input(p(json!(["integer", "string"])), p(json!(["number", "string"]))));
    none(&input(p(json!("string")), obj(json!({"n": {}}), &[])));
}

#[test]
fn input_enum_removal_is_breaking() {
    let p = |e: Value| obj(json!({"c": {"type": "string", "enum": e}}), &[]);
    let changes = input(p(json!(["a", "b", "c"])), p(json!(["a", "d"])));
    one(&changes, Breaking, "/inputSchema/properties/c/enum");
    assert!(changes[0].message.contains("\"b\", \"c\""), "{}", changes[0].message);
    // 原本不限定：新增 enum 即删除了其他取值
    one(&input(obj(json!({"c": {"type": "string"}}), &[]), p(json!(["a"]))), Breaking, "/inputSchema/properties/c/enum");
}

#[test]
fn input_enum_addition_is_compatible() {
    let p = |e: Value| obj(json!({"c": {"enum": e}}), &[]);
    none(&input(p(json!(["a"])), p(json!(["a", "b"]))));
    none(&input(p(json!(["a"])), obj(json!({"c": {}}), &[])));
}

#[test]
fn input_additional_properties_false_is_breaking() {
    let mut old = obj(json!({}), &[]);
    let mut new = old.clone();
    new["additionalProperties"] = json!(false);
    one(&input(old.clone(), new.clone()), Breaking, "/inputSchema/additionalProperties");
    old["additionalProperties"] = json!(true);
    one(&input(old, new.clone()), Breaking, "/inputSchema/additionalProperties");
    // false → 允许：放宽
    none(&input(new, obj(json!({}), &[])));
}

// ---------------------------------------------------------------------------
// inputSchema：可能破坏
// ---------------------------------------------------------------------------

#[test]
fn input_constraint_added_or_tightened_is_warning() {
    let p = |c: Value| {
        let mut s = json!({"type": "string"});
        for (k, v) in c.as_object().unwrap() {
            s[k] = v.clone();
        }
        obj(json!({"x": s}), &[])
    };
    let cases = [
        ("minimum", json!(1), json!(2)),
        ("exclusiveMinimum", json!(1), json!(2)),
        ("minLength", json!(1), json!(3)),
        ("minItems", json!(0), json!(1)),
        ("maximum", json!(10), json!(9)),
        ("exclusiveMaximum", json!(10), json!(9)),
        ("maxLength", json!(64), json!(32)),
        ("maxItems", json!(5), json!(4)),
        ("pattern", json!("^a"), json!("^b")),
        ("format", json!("date"), json!("date-time")),
        ("minProperties", json!(0), json!(1)),
        ("maxProperties", json!(5), json!(4)),
        ("const", json!("a"), json!("b")),
        ("multipleOf", json!(1), json!(2)),
        ("uniqueItems", json!(false), json!(true)),
        ("patternProperties", json!({"^a": {}}), json!({"^b": {}})),
        ("prefixItems", json!([{"type": "string"}]), json!([{"type": "number"}])),
        ("dependentRequired", json!({"a": ["b"]}), json!({"a": ["c"]})),
    ];
    for (kw, before, after) in cases {
        let path = format!("/inputSchema/properties/x/{kw}");
        one(&input(p(json!({})), p(json!({kw: after.clone()}))), Warning, &path);
        one(&input(p(json!({kw: before.clone()})), p(json!({kw: after.clone()}))), Warning, &path);
        // 反向（放宽，只对上下限类数值约束；multipleOf 的松紧无法静态比较）与删除：兼容
        if before.is_number() && kw != "multipleOf" {
            none(&input(p(json!({kw: after.clone()})), p(json!({kw: before}))));
        }
        none(&input(p(json!({kw: after})), p(json!({}))));
    }
}

#[test]
fn combinator_change_is_warning_without_expanding() {
    for kw in ["$ref", "oneOf", "anyOf", "allOf", "not", "if", "then", "else", "$defs", "definitions"] {
        let old = json!({"type": "object", kw: [{"type": "string"}]});
        let new = json!({"type": "object", kw: [{"type": "number"}]});
        one(&input(old.clone(), new.clone()), Warning, &format!("/inputSchema/{kw}"));
        one(&output(old, new), Warning, &format!("/outputSchema/{kw}"));
    }
    // 新增组合关键字也算变化
    one(&input(json!({"type": "object"}), json!({"type": "object", "anyOf": [{}]})), Warning, "/inputSchema/anyOf");
}

// ---------------------------------------------------------------------------
// inputSchema：兼容
// ---------------------------------------------------------------------------

#[test]
fn input_compatible_changes_are_not_listed() {
    let old = obj(json!({"to": {"type": "string", "description": "a", "title": "A", "default": "x"}}), &["to"]);
    let new = obj(
        json!({
            "to": {"type": "string", "description": "b", "title": "B", "default": "y", "deprecated": true},
            "cc": {"type": "string"}
        }),
        &[],
    );
    none(&input(old, new));
}

// ---------------------------------------------------------------------------
// outputSchema
// ---------------------------------------------------------------------------

#[test]
fn output_removed_property_is_breaking() {
    let old = obj(json!({"id": {"type": "string"}, "total": {"type": "number"}}), &["id", "total"]);
    let new = obj(json!({"id": {"type": "string"}}), &["id"]);
    // 删除属性只记一条（不再另记「从 required 中移除」）
    one(&output(old, new), Breaking, "/outputSchema/properties/total");
}

#[test]
fn output_type_change_unless_subset_is_breaking() {
    let p = |t: Value| obj(json!({"v": {"type": t}}), &[]);
    one(&output(p(json!("string")), p(json!(["string", "null"]))), Breaking, "/outputSchema/properties/v/type");
    one(&output(p(json!("integer")), p(json!("number"))), Breaking, "/outputSchema/properties/v/type");
    one(&output(p(json!("string")), obj(json!({"v": {}}), &[])), Breaking, "/outputSchema/properties/v/type");
    // 收窄为子集：兼容
    none(&output(p(json!(["string", "null"])), p(json!("string"))));
    none(&output(p(json!("number")), p(json!("integer"))));
}

#[test]
fn output_required_removed_is_breaking() {
    let props = json!({"id": {"type": "string"}});
    one(&output(obj(props.clone(), &["id"]), obj(props, &[])), Breaking, "/outputSchema/properties/id");
}

#[test]
fn output_enum_addition_is_warning() {
    let p = |e: Value| obj(json!({"s": {"enum": e}}), &[]);
    one(&output(p(json!(["a"])), p(json!(["a", "b"]))), Warning, "/outputSchema/properties/s/enum");
    one(&output(p(json!(["a"])), obj(json!({"s": {}}), &[])), Warning, "/outputSchema/properties/s/enum");
    // 删除取值：读取方不受影响
    none(&output(p(json!(["a", "b"])), p(json!(["a"]))));
}

#[test]
fn output_compatible_changes_are_not_listed() {
    let old = obj(json!({"id": {"type": "string", "description": "a"}}), &[]);
    let new = obj(json!({"id": {"type": "string", "description": "b", "minLength": 1}, "extra": {}}), &["id", "extra"]);
    none(&output(old, new));
    // outputSchema 不检查 additionalProperties
    none(&output(json!({"type": "object"}), json!({"type": "object", "additionalProperties": false})));
}

#[test]
fn output_schema_presence() {
    let base = tool("t", json!({"type": "object"}));
    let declared = with(base.clone(), json!({"outputSchema": {"type": "object"}}));
    none(&compare_tool(&base, &declared));
    one(&compare_tool(&declared, &base), Warning, "/outputSchema");
}

// ---------------------------------------------------------------------------
// 递归、数组、深度上限、非对象 schema
// ---------------------------------------------------------------------------

#[test]
fn recursion_into_nested_objects_and_arrays() {
    let addr = |req: &[&str]| obj(json!({"city": {"type": "string"}, "zip": {"type": "string"}}), req);
    let old = obj(json!({"items": {"type": "array", "items": addr(&[])}}), &[]);
    let new = obj(json!({"items": {"type": "array", "items": addr(&["zip"])}}), &[]);
    one(&input(old, new), Breaking, "/inputSchema/properties/items/items/properties/zip");
    // 数组元素类型在输出中扩大
    let arr = |t: &str| obj(json!({"tags": {"type": "array", "items": {"type": t}}}), &[]);
    one(&output(arr("integer"), arr("number")), Breaking, "/outputSchema/properties/tags/items/type");
    // 输入新增 items 限制：原来任意元素，现在只收字符串
    let old = obj(json!({"tags": {"type": "array"}}), &[]);
    one(&input(old, arr("string")), Breaking, "/inputSchema/properties/tags/items/type");
}

#[test]
fn pointer_escapes_special_characters() {
    let old = obj(json!({"a/b~c": {"type": "string"}}), &[]);
    one(&input(old, obj(json!({}), &[])), Breaking, "/inputSchema/properties/a~1b~0c");
}

#[test]
fn depth_limit_stops_with_warning() {
    fn nest(depth: usize, leaf: &str) -> Value {
        (0..depth).fold(json!({"type": leaf}), |inner, _| json!({"type": "object", "properties": {"n": inner}}))
    }
    // 上限以内：叶子的收窄被找到
    let within = input(nest(MAX_SCHEMA_DEPTH, "number"), nest(MAX_SCHEMA_DEPTH, "integer"));
    assert_eq!(within.len(), 1, "{within:#?}");
    assert_eq!(within[0].level, Breaking);
    // 超出：一条 Warning，不 panic
    let deep = input(nest(MAX_SCHEMA_DEPTH + 5, "number"), nest(MAX_SCHEMA_DEPTH + 5, "integer"));
    assert_eq!(deep.len(), 1, "{deep:#?}");
    assert_eq!(deep[0].level, Warning);
    assert!(deep[0].message.contains("未完整比较"), "{}", deep[0].message);
    assert_eq!(deep[0].path.matches("/properties/n").count(), MAX_SCHEMA_DEPTH + 1);
}

#[test]
fn boolean_and_non_object_schemas() {
    one(&input(obj(json!({"x": true}), &[]), obj(json!({"x": false}), &[])), Breaking, "/inputSchema/properties/x");
    none(&input(obj(json!({"x": false}), &[]), obj(json!({"x": {"type": "string"}}), &[])));
    // true 等价于空 schema
    none(&input(obj(json!({"x": true}), &[]), obj(json!({"x": {}}), &[])));
    // 旧式元组 items：不展开，记 Warning
    let tuple = |t: &str| obj(json!({"p": {"type": "array", "items": [{"type": t}]}}), &[]);
    one(&input(tuple("string"), tuple("number")), Warning, "/inputSchema/properties/p/items");
    // 畸形 type（非字符串 / 数组）视为任意
    none(&input(obj(json!({"x": {"type": 1}}), &[]), obj(json!({"x": {}}), &[])));
}

// ---------------------------------------------------------------------------
// 工具本身
// ---------------------------------------------------------------------------

#[test]
fn surface_app_to_view_is_breaking() {
    let base = tool("t", json!({"type": "object"}));
    let view = with(base.clone(), json!({"surface": "view"}));
    one(&compare_tool(&base, &view), Breaking, "/surface");
    none(&compare_tool(&view, &base));
}

#[test]
fn read_only_to_non_read_only_is_warning() {
    let base = tool("t", json!({"type": "object"}));
    let ro = with(base.clone(), json!({"annotations": {"readOnlyHint": true}}));
    let rw = with(base.clone(), json!({"annotations": {"readOnlyHint": false}}));
    one(&compare_tool(&ro, &rw), Warning, "/annotations/readOnlyHint");
    none(&compare_tool(&rw, &ro));
    // 生效注解：risk read（推导只读）→ 显式非只读
    let read = with(base.clone(), json!({"risk": "read"}));
    let changes = compare_tool(&read, &rw);
    assert!(changes.iter().any(|c| c.path == "/annotations/readOnlyHint"), "{changes:#?}");
}

#[test]
fn risk_increase_is_warning() {
    let base = tool("t", json!({"type": "object"}));
    let order = ["read", "write", "destructive", "payment", "os-sensitive"];
    for pair in order.windows(2) {
        let low = with(base.clone(), json!({"risk": pair[0], "annotations": {"readOnlyHint": false}}));
        let high = with(base.clone(), json!({"risk": pair[1], "annotations": {"readOnlyHint": false}}));
        one(&compare_tool(&low, &high), Warning, "/risk");
        none(&compare_tool(&high, &low));
    }
}

#[test]
fn tool_metadata_changes_are_compatible() {
    let base = tool("t", json!({"type": "object"}));
    let changed = with(
        base.clone(),
        json!({
            "description": "新描述", "title": "标题", "implements": ["message.send@1"],
            "cache": {"ttlMs": 1000}, "deprecated": {"message": "改用 t2", "replacement": "t2"}
        }),
    );
    none(&compare_tool(&base, &changed));
}

// ---------------------------------------------------------------------------
// compare_tools
// ---------------------------------------------------------------------------

#[test]
fn compare_tools_removal_and_ordering() {
    let s = json!({"type": "object"});
    let deprecated = with(tool("b.old", s.clone()), json!({"deprecated": {"message": "改用 b.new"}}));
    let old = vec![tool("z", s.clone()), tool("a", s.clone()), deprecated, tool("same", s.clone())];
    let new = vec![
        tool("same", s.clone()),
        with(tool("z", s.clone()), json!({"surface": "view"})),
        tool("b.new", s),
    ];
    let result = compare_tools(&old, &new);
    let names: Vec<_> = result.iter().map(|c| c.tool.as_str()).collect();
    assert_eq!(names, ["a", "b.old", "z"], "新增与无变化的工具不列出、按名排序");
    assert!(result[0].removed && result[0].has_breaking(), "未经弃用的删除为破坏");
    assert!(result[1].removed && result[1].changes.is_empty(), "已弃用的删除只提示");
    assert!(!result[2].removed && result[2].has_breaking());
}

#[test]
fn serde_shape() {
    let c = ToolChanges {
        tool: "t".into(),
        removed: false,
        changes: vec![SchemaChange::new(Warning, "/risk", "m")],
    };
    assert_eq!(
        serde_json::to_value(&c).unwrap(),
        json!({"tool": "t", "removed": false, "changes": [{"level": "warning", "path": "/risk", "message": "m"}]})
    );
    assert_eq!(serde_json::to_value(Breaking).unwrap(), json!("breaking"));
    assert_eq!(Breaking.as_str(), "breaking");
}
