//! `inputSchema` / `outputSchema` 的逐层比较（spec/manifest.md 第 6 节前两行）。
//!
//! 从 schema 根开始，沿 `properties` 递归进入对象、沿 `items` 进入数组元素；组合关键字不展开。
//! 方向决定规则：`inputSchema` 由调用方写入（收窄即破坏），`outputSchema` 由调用方读取（扩张即破坏）。

use serde_json::{Map, Value};

use super::{ChangeLevel, SchemaChange};

/// 递归深度上限（`properties` / `items` 每进入一层加一）：超出时记一条 Warning、不再深入（B-07）。
pub const MAX_SCHEMA_DEPTH: usize = 32;

/// 组合关键字（含 `$ref` 指向的定义区）：内容变化记为可能破坏，不展开比较。
const COMBINATORS: [&str; 10] = ["$ref", "oneOf", "anyOf", "allOf", "not", "if", "then", "else", "$defs", "definitions"];
/// 下限类约束：新增或调大即收紧。
const LOWER_BOUNDS: [&str; 5] = ["minimum", "exclusiveMinimum", "minLength", "minItems", "minProperties"];
/// 上限类约束：新增或调小即收紧。
const UPPER_BOUNDS: [&str; 5] = ["maximum", "exclusiveMaximum", "maxLength", "maxItems", "maxProperties"];
/// 无法比较松紧的约束：新增或改变即视为收紧（可能破坏）。
///
/// @why `additionalProperties` 为 schema 形式时内容的变化不在此列：变为 `false` 已单独判为破坏性，schema 形式的松紧无法静态比较，
///      交给组合关键字之外的人工审阅（spec/manifest.md 第 6 节未列）。
const OPAQUE_CONSTRAINTS: [&str; 8] =
    ["pattern", "format", "const", "multipleOf", "uniqueItems", "patternProperties", "prefixItems", "dependentRequired"];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Direction {
    /// 调用方传入（`inputSchema`）。
    Input,
    /// 调用方读取（`outputSchema`）。
    Output,
}

/// 比较 `inputSchema`，变化追加到 `out`。
pub(super) fn compare_input(old: &Value, new: &Value, out: &mut Vec<SchemaChange>) {
    Walker { dir: Direction::Input, out }.compare(old, new, "/inputSchema", 0);
}

/// 比较 `outputSchema`，变化追加到 `out`。
pub(super) fn compare_output(old: &Value, new: &Value, out: &mut Vec<SchemaChange>) {
    Walker { dir: Direction::Output, out }.compare(old, new, "/outputSchema", 0);
}

struct Walker<'a> {
    dir: Direction,
    out: &'a mut Vec<SchemaChange>,
}

impl Walker<'_> {
    fn push(&mut self, level: ChangeLevel, path: String, message: String) {
        self.out.push(SchemaChange::new(level, path, message));
    }

    fn compare(&mut self, old: &Value, new: &Value, path: &str, depth: usize) {
        if old == new {
            return;
        }
        if depth > MAX_SCHEMA_DEPTH {
            let msg = format!("schema 嵌套超过 {MAX_SCHEMA_DEPTH} 层，未完整比较");
            self.push(ChangeLevel::Warning, path.to_owned(), msg);
            return;
        }
        let empty = Map::new();
        match (as_schema(old, &empty), as_schema(new, &empty)) {
            (Some(o), Some(n)) => self.compare_objects(o, n, path, depth),
            _ => self.compare_non_object(old, new, path),
        }
    }

    /// 至少一侧是 `false` 或不是 schema（如旧式元组 `items: [...]`）。
    fn compare_non_object(&mut self, old: &Value, new: &Value, path: &str) {
        let path = path.to_owned();
        match self.dir {
            Direction::Input if *new == Value::Bool(false) => {
                self.push(ChangeLevel::Breaking, path, "schema 变为 false：不再接受任何值".into());
            }
            Direction::Input if *old == Value::Bool(false) => {}
            _ => self.push(ChangeLevel::Warning, path, "schema 形式变化（布尔 schema 或非对象），未完整比较".into()),
        }
    }

    fn compare_objects(&mut self, o: &Map<String, Value>, n: &Map<String, Value>, path: &str, depth: usize) {
        self.combinators(o, n, path);
        self.types(o, n, path);
        self.enums(o, n, path);
        if self.dir == Direction::Input {
            self.additional_properties(o, n, path);
            self.constraints(o, n, path);
        }
        self.properties(o, n, path, depth);
        self.items(o, n, path, depth);
    }

    fn combinators(&mut self, o: &Map<String, Value>, n: &Map<String, Value>, path: &str) {
        for kw in COMBINATORS {
            if o.get(kw) != n.get(kw) {
                let msg = format!("组合关键字 `{kw}` 内容变化（未展开比较）");
                self.push(ChangeLevel::Warning, child(path, kw), msg);
            }
        }
    }

    /// `type` 取值集合：输入收窄为破坏；输出不是旧集合的子集为破坏。
    fn types(&mut self, o: &Map<String, Value>, n: &Map<String, Value>, path: &str) {
        let (old, new) = (type_set(o), type_set(n));
        let (narrower, wider, what) = match self.dir {
            Direction::Input => (&new, &old, "收窄"),
            Direction::Output => (&old, &new, "扩大（不是原集合的子集）"),
        };
        if !covers(narrower, wider) {
            let msg = format!("type 取值集合{what}：{} → {}", show_types(&old), show_types(&new));
            self.push(ChangeLevel::Breaking, child(path, "type"), msg);
        }
    }

    /// `enum`：输入删除取值（或新增 `enum` 限制）为破坏；输出增加取值（或取消限制）为可能破坏。
    fn enums(&mut self, o: &Map<String, Value>, n: &Map<String, Value>, path: &str) {
        let (old, new) = (enum_values(o), enum_values(n));
        let path = child(path, "enum");
        match (self.dir, old, new) {
            (Direction::Input, Some(a), Some(b)) => {
                let removed = missing(a, b);
                if !removed.is_empty() {
                    self.push(ChangeLevel::Breaking, path, format!("enum 删除取值：{}", show_values(&removed)));
                }
            }
            (Direction::Input, None, Some(b)) => {
                self.push(ChangeLevel::Breaking, path, format!("新增 enum 限制：只接受 {}", show_values(&refs(b))));
            }
            (Direction::Output, Some(a), Some(b)) => {
                let added = missing(b, a);
                if !added.is_empty() {
                    self.push(ChangeLevel::Warning, path, format!("enum 增加取值：{}", show_values(&added)));
                }
            }
            (Direction::Output, Some(_), None) => {
                self.push(ChangeLevel::Warning, path, "取消 enum 限制：结果可能出现任意取值".into());
            }
            _ => {}
        }
    }

    fn additional_properties(&mut self, o: &Map<String, Value>, n: &Map<String, Value>, path: &str) {
        let is_false = |m: &Map<String, Value>| m.get("additionalProperties") == Some(&Value::Bool(false));
        if !is_false(o) && is_false(n) {
            let msg = "additionalProperties 变为 false：传入未声明的参数将被拒绝".to_owned();
            self.push(ChangeLevel::Breaking, child(path, "additionalProperties"), msg);
        }
    }

    /// 输入约束新增或收紧为可能破坏（放宽、删除不列）。
    fn constraints(&mut self, o: &Map<String, Value>, n: &Map<String, Value>, path: &str) {
        let bounds = LOWER_BOUNDS.iter().map(|k| (*k, Some(true))).chain(UPPER_BOUNDS.iter().map(|k| (*k, Some(false))));
        let checks = bounds.chain(OPAQUE_CONSTRAINTS.iter().map(|k| (*k, None)));
        for (kw, lower) in checks {
            let Some(after) = n.get(kw) else { continue };
            let message = match o.get(kw) {
                None => format!("新增约束 {kw} = {after}"),
                Some(before) if before == after => continue,
                Some(before) => match (lower, before.as_f64(), after.as_f64()) {
                    (Some(true), Some(b), Some(a)) if a <= b => continue,
                    (Some(false), Some(b), Some(a)) if a >= b => continue,
                    _ => format!("约束 {kw} 收紧或改变：{before} → {after}"),
                },
            };
            self.push(ChangeLevel::Warning, child(path, kw), message);
        }
    }

    /// `required` 与属性增删，并递归进入新旧都有的属性。
    fn properties(&mut self, o: &Map<String, Value>, n: &Map<String, Value>, path: &str, depth: usize) {
        let empty = Map::new();
        let old_props = o.get("properties").and_then(Value::as_object).unwrap_or(&empty);
        let new_props = n.get("properties").and_then(Value::as_object).unwrap_or(&empty);
        let (old_req, new_req) = (required(o), required(n));
        let prop_path = |name: &str| child(&child(path, "properties"), name);
        for (name, before) in old_props {
            match new_props.get(name) {
                None => {
                    let what = if self.dir == Direction::Input { "参数" } else { "属性" };
                    self.push(ChangeLevel::Breaking, prop_path(name), format!("删除{what} `{name}`"));
                }
                Some(after) => self.compare(before, after, &prop_path(name), depth + 1),
            }
        }
        match self.dir {
            Direction::Input => {
                for name in new_req.iter().filter(|r| !old_req.contains(r)) {
                    self.push(ChangeLevel::Breaking, prop_path(name), format!("新增必填参数 `{name}`"));
                }
            }
            Direction::Output => {
                let deleted = |name: &str| old_props.contains_key(name) && !new_props.contains_key(name);
                for name in old_req.iter().filter(|r| !new_req.contains(r) && !deleted(r)) {
                    let msg = format!("属性 `{name}` 从 required 中移除：结果中可能不再出现");
                    self.push(ChangeLevel::Breaking, prop_path(name), msg);
                }
            }
        }
    }

    /// 数组元素：缺省 `items` 视为任意（`true`）。
    fn items(&mut self, o: &Map<String, Value>, n: &Map<String, Value>, path: &str, depth: usize) {
        let any = Value::Bool(true);
        match (o.get("items"), n.get("items")) {
            (None, None) => {}
            (before, after) => self.compare(before.unwrap_or(&any), after.unwrap_or(&any), &child(path, "items"), depth + 1),
        }
    }
}

/// 对象 schema 原样返回；`true` 等价于空 schema；`false` 与非 schema 为 `None`。
fn as_schema<'a>(v: &'a Value, empty: &'a Map<String, Value>) -> Option<&'a Map<String, Value>> {
    match v {
        Value::Object(m) => Some(m),
        Value::Bool(true) => Some(empty),
        _ => None,
    }
}

/// `type` 取值集合：字符串视为单元素；缺省或格式不对视为任意（`None`）。
fn type_set(m: &Map<String, Value>) -> Option<Vec<&str>> {
    match m.get("type")? {
        Value::String(t) => Some(vec![t.as_str()]),
        Value::Array(items) => items.iter().map(Value::as_str).collect(),
        _ => None,
    }
}

/// `outer` 是否包含 `inner` 的每个取值（`integer` 视为 `number` 的子集；`None` 为任意）。
fn covers(outer: &Option<Vec<&str>>, inner: &Option<Vec<&str>>) -> bool {
    let Some(outer) = outer else { return true };
    let Some(inner) = inner else { return false };
    inner.iter().all(|t| outer.contains(t) || (*t == "integer" && outer.contains(&"number")))
}

fn show_types(set: &Option<Vec<&str>>) -> String {
    set.as_ref().map_or_else(|| "任意".to_owned(), |s| format!("[{}]", s.join(", ")))
}

fn enum_values(m: &Map<String, Value>) -> Option<&Vec<Value>> {
    m.get("enum")?.as_array()
}

/// `from` 中不在 `to` 里的取值。
fn missing<'a>(from: &'a [Value], to: &[Value]) -> Vec<&'a Value> {
    from.iter().filter(|v| !to.contains(v)).collect()
}

fn refs(values: &[Value]) -> Vec<&Value> {
    values.iter().collect()
}

fn show_values(values: &[&Value]) -> String {
    values.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(", ")
}

fn required(m: &Map<String, Value>) -> Vec<&str> {
    m.get("required").and_then(Value::as_array).map(|r| r.iter().filter_map(Value::as_str).collect()).unwrap_or_default()
}

/// JSON Pointer 拼接（RFC 6901 转义：`~` → `~0`，`/` → `~1`）。
fn child(path: &str, key: &str) -> String {
    format!("{path}/{}", key.replace('~', "~0").replace('/', "~1"))
}
