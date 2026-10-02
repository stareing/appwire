//! 一致性用例（`conformance/cases/*.json`，格式见 conformance/README.md）：把用例的 `host` 部分翻译成 fake_host 的
//! 命令行参数，运行结束后按 `expect` / `contains` / `absent` 核对 fake_host 打印的各行，给出结论。
//!
//! @why 核对逻辑只有这一份（Rust），各语言的 runner 只负责按用例的 `app` 部分注册工具并连接，不各自实现匹配器。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

/// 一个已加载的用例。
pub struct Case {
    pub id: String,
    pub doc: Value,
    /// 用例目录的上一级（`conformance/`），`divergences/` 所在位置。
    pub root: PathBuf,
}

impl Case {
    /// @error 文件不可读、不是 JSON 对象或缺少 `id` 时返回说明。
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("无法读取用例 {}：{e}", path.display()))?;
        let doc: Value = serde_json::from_str(&text).map_err(|e| format!("用例 {} 不是合法 JSON：{e}", path.display()))?;
        let id = doc["id"].as_str().ok_or_else(|| format!("用例 {} 缺少 id", path.display()))?.to_owned();
        let root = path
            .parent()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        Ok(Self { id, doc, root })
    }

    /// 用例 `host` 部分 → fake_host 命令行参数（与手写命令行走同一个解析器）。
    ///
    /// @error 未知操作或字段类型不对时返回说明。
    pub fn host_args(&self) -> Result<Vec<String>, String> {
        let host = &self.doc["host"];
        let mut args = Vec::new();
        let flag_u64 = |args: &mut Vec<String>, key: &str, flag: &str| -> Result<(), String> {
            match &host[key] {
                Value::Null => Ok(()),
                v => {
                    let n = v.as_u64().ok_or_else(|| format!("host.{key} 应为非负整数"))?;
                    args.extend([flag.to_owned(), n.to_string()]);
                    Ok(())
                }
            }
        };
        if host["toolInfo"].as_bool() == Some(true) {
            args.push("--tool-info".to_owned());
        }
        if host["trace"].as_bool() != Some(false) {
            args.push("--trace".to_owned());
        }
        flag_u64(&mut args, "leaseMs", "--lease-ms")?;
        flag_u64(&mut args, "rejectSleepMs", "--reject-sleep")?;
        flag_u64(&mut args, "timeoutMs", "--timeout-ms")?;
        let ops = host["ops"].as_array().cloned().unwrap_or_default();
        for op in &ops {
            args.extend(op_args(op)?);
        }
        Ok(args)
    }

    /// 已登记的已知偏差（`conformance/divergences/<sdk>.json`：`{ <caseId>: 原因 }`）。
    pub fn divergence(&self, sdk: &str) -> Option<String> {
        let path = self.root.join("divergences").join(format!("{sdk}.json"));
        let text = std::fs::read_to_string(path).ok()?;
        let doc: Value = serde_json::from_str(&text).ok()?;
        doc[&self.id].as_str().map(str::to_owned)
    }
}

fn op_args(op: &Value) -> Result<Vec<String>, String> {
    let obj = op.as_object().ok_or_else(|| format!("host.ops 的元素应为对象：{op}"))?;
    let num = |key: &str| -> Result<Option<String>, String> {
        match obj.get(key) {
            None => Ok(None),
            Some(v) => v
                .as_u64()
                .map(|n| Some(n.to_string()))
                .ok_or_else(|| format!("{key} 应为非负整数：{op}")),
        }
    };
    let s = |v: &Value| v.as_str().map(str::to_owned).ok_or_else(|| format!("应为字符串：{op}"));
    if let Some(name) = obj.get("invoke") {
        let mut a = vec!["--invoke".to_owned(), s(name)?];
        if let Some(args) = obj.get("args") {
            a.extend(["--args".to_owned(), args.to_string()]);
        }
        if let Some(id) = obj.get("callId") {
            a.extend(["--call-id".to_owned(), s(id)?]);
        }
        if let Some(key) = obj.get("idempotencyKey") {
            a.extend(["--idempotency-key".to_owned(), s(key)?]);
        }
        if let Some(n) = num("timeoutMs")? {
            a.extend(["--invoke-timeout-ms".to_owned(), n]);
        }
        if let Some(n) = num("cancelAfterMs")? {
            a.extend(["--cancel-after-ms".to_owned(), n]);
        }
        return Ok(a);
    }
    if let Some(page) = obj.get("navigate") {
        let mut a = vec!["--navigate".to_owned(), s(page)?];
        if let Some(params) = obj.get("params") {
            a.extend(["--nav-params".to_owned(), params.to_string()]);
        }
        return Ok(a);
    }
    if let Some(name) = obj.get("read") {
        return Ok(vec!["--read".to_owned(), s(name)?]);
    }
    if obj.contains_key("catalog") {
        return Ok(vec!["--catalog".to_owned(), num("catalog")?.unwrap_or_else(|| "0".to_owned())]);
    }
    if let Some(n) = num("delay")? {
        return Ok(vec!["--delay".to_owned(), n]);
    }
    if obj.contains_key("awaitSleep") {
        return Ok(vec!["--await-sleep".to_owned()]);
    }
    if obj.contains_key("wake") {
        return Ok(vec!["--wake".to_owned()]);
    }
    Err(format!("未知的 host 操作：{op}"))
}

// ---------------------------------------------------------------------------
// 匹配
// ---------------------------------------------------------------------------

/// 匹配过程中绑定的变量（`{"$bind": "名称"}`）。
type Bindings = BTreeMap<String, Value>;

/// `pattern` 是否匹配 `value`（规则见 conformance/README.md）。匹配成功时把新绑定写入 `binds`。
fn matches(pattern: &Value, value: Option<&Value>, binds: &mut Bindings) -> bool {
    if let Some(op) = operator(pattern) {
        return match_operator(op, value, binds);
    }
    let Some(value) = value else { return false };
    match (pattern, value) {
        (Value::Object(p), Value::Object(v)) => p.iter().all(|(k, pv)| matches(pv, v.get(k), binds)),
        (Value::Array(p), Value::Array(v)) => {
            p.len() == v.len() && p.iter().zip(v).all(|(pe, ve)| matches(pe, Some(ve), binds))
        }
        (Value::Number(p), Value::Number(v)) => p.as_f64() == v.as_f64(),
        (p, v) => p == v,
    }
}

/// 形如 `{"$xxx": …}` 的单键对象是操作符。
fn operator(pattern: &Value) -> Option<(&str, &Value)> {
    let obj = pattern.as_object()?;
    if obj.len() != 1 {
        return None;
    }
    let (k, v) = obj.iter().next()?;
    k.starts_with('$').then_some((k.as_str(), v))
}

fn match_operator((op, arg): (&str, &Value), value: Option<&Value>, binds: &mut Bindings) -> bool {
    match op {
        "$absent" => value.is_none(),
        "$any" => value.is_some(),
        "$type" => value.is_some_and(|v| arg.as_str() == Some(json_type(v))),
        "$contains" => value
            .and_then(Value::as_str)
            .zip(arg.as_str())
            .is_some_and(|(v, needle)| v.contains(needle)),
        "$has" => {
            let (Some(items), Some(wanted)) = (value.and_then(Value::as_array), arg.as_array()) else { return false };
            wanted.iter().all(|w| items.iter().any(|it| matches(w, Some(it), binds)))
        }
        "$oneOf" => arg
            .as_array()
            .is_some_and(|alts| alts.iter().any(|a| matches(a, value, &mut binds.clone()))),
        "$bind" => {
            let (Some(name), Some(v)) = (arg.as_str(), value) else { return false };
            match binds.get(name) {
                Some(bound) => bound == v,
                None => {
                    binds.insert(name.to_owned(), v.clone());
                    true
                }
            }
        }
        _ => false,
    }
}

fn json_type(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// 按用例核对记录下来的各行，返回不满足的期望（空 = 通过）。
pub fn check(case: &Value, lines: &[Value]) -> Vec<String> {
    let mut failures = Vec::new();
    let mut binds = Bindings::new();
    let list = |key: &str| case[key].as_array().cloned().unwrap_or_default();

    // expect：按顺序依次出现（中间可夹其他行）。
    let mut from = 0;
    for (i, pattern) in list("expect").iter().enumerate() {
        let found = lines[from..].iter().position(|l| {
            let mut trial = binds.clone();
            let ok = matches(pattern, Some(l), &mut trial);
            if ok {
                binds = trial;
            }
            ok
        });
        match found {
            Some(at) => from += at + 1,
            None => failures.push(format!("expect[{i}] 未按顺序出现：{pattern}")),
        }
    }
    // contains：任意位置出现。
    for (i, pattern) in list("contains").iter().enumerate() {
        let ok = lines.iter().any(|l| {
            let mut trial = binds.clone();
            let ok = matches(pattern, Some(l), &mut trial);
            if ok {
                binds = trial;
            }
            ok
        });
        if !ok {
            failures.push(format!("contains[{i}] 未出现：{pattern}"));
        }
    }
    // absent：不得出现。
    for (i, pattern) in list("absent").iter().enumerate() {
        if let Some(l) = lines.iter().find(|l| matches(pattern, Some(l), &mut binds.clone())) {
            failures.push(format!("absent[{i}] 出现了：{l}"));
        }
    }
    failures
}

/// 结论：`pass` / `fail`，登记了偏差时为 `xfail`（仍失败）/ `xpass`（已不再偏差，应删除登记）。
pub fn verdict(case: &Case, sdk: Option<&str>, run_error: Option<String>, lines: &[Value]) -> Value {
    let mut failures = check(&case.doc, lines);
    if let Some(e) = run_error {
        failures.insert(0, format!("运行出错：{e}"));
    }
    let divergence = sdk.and_then(|s| case.divergence(s));
    let status = match (failures.is_empty(), divergence.is_some()) {
        (true, false) => "pass",
        (true, true) => "xpass",
        (false, true) => "xfail",
        (false, false) => "fail",
    };
    let mut v = Map::new();
    v.insert("type".into(), json!("verdict"));
    v.insert("case".into(), json!(case.id));
    v.insert("sdk".into(), json!(sdk));
    v.insert("status".into(), json!(status));
    v.insert("failures".into(), json!(failures));
    if let Some(d) = divergence {
        v.insert("divergence".into(), json!(d));
    }
    Value::Object(v)
}

/// 跳过（runner 不支持用例需要的能力）时的结论。
pub fn skip_verdict(case: &Case, sdk: Option<&str>, reason: &str) -> Value {
    json!({ "type": "verdict", "case": case.id, "sdk": sdk, "status": "skip", "reason": reason, "failures": [] })
}

/// 把结论与完整记录写到 `<dir>/<sdk>/<caseId>.json`，供 `conformance/matrix.mjs` 汇总。
pub fn write_report(dir: &Path, sdk: &str, case: &Case, verdict: &Value, lines: &[Value]) -> Result<(), String> {
    let sdk_dir = dir.join(sdk);
    std::fs::create_dir_all(&sdk_dir).map_err(|e| format!("无法创建 {}：{e}", sdk_dir.display()))?;
    let mut report = verdict.clone();
    report["transcript"] = Value::Array(lines.to_vec());
    let path = sdk_dir.join(format!("{}.json", case.id));
    let text = serde_json::to_string_pretty(&report).unwrap_or_default();
    std::fs::write(&path, text).map_err(|e| format!("无法写入 {}：{e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(p: Value, v: Value) -> bool {
        matches(&p, Some(&v), &mut Bindings::new())
    }

    #[test]
    fn matches_partial_objects_and_operators() {
        assert!(ok(json!({"a": 1}), json!({"a": 1.0, "b": 2})));
        assert!(!ok(json!({"a": 1}), json!({"b": 2})));
        assert!(ok(json!({"a": {"$absent": true}}), json!({"b": 2})));
        assert!(!ok(json!({"a": {"$absent": true}}), json!({"a": null})));
        assert!(ok(json!({"a": {"$type": "string"}}), json!({"a": "x"})));
        assert!(ok(json!({"a": {"$contains": "lo"}}), json!({"a": "hello"})));
        assert!(ok(json!({"a": {"$oneOf": [1, null]}}), json!({"a": null})));
        assert!(ok(json!([1, {"x": 1}]), json!([1, {"x": 1, "y": 2}])));
        assert!(ok(json!({"$has": [2, {"x": 1}]}), json!([{"x": 1, "y": 2}, 3, 2])));
        assert!(!ok(json!({"$has": [4]}), json!([1, 2])));
        assert!(!ok(json!([1]), json!([1, 2])));
    }

    #[test]
    fn checks_order_bindings_and_absence() {
        let case = json!({
            "expect": [{"t": "a", "h": {"$bind": "h"}}, {"t": "b", "h": {"$bind": "h"}}],
            "contains": [{"t": "c"}],
            "absent": [{"t": "z"}]
        });
        let good = [json!({"t": "a", "h": 1}), json!({"t": "c"}), json!({"t": "b", "h": 1})];
        assert!(check(&case, &good).is_empty());
        let bad = [json!({"t": "b", "h": 1}), json!({"t": "a", "h": 2}), json!({"t": "b", "h": 1}), json!({"t": "z"})];
        let f = check(&case, &bad);
        assert_eq!(f.len(), 3, "{f:?}");
    }

    #[test]
    fn translates_host_ops() {
        let case = Case {
            id: "x".into(),
            root: PathBuf::from("."),
            doc: json!({"host": {"toolInfo": true, "leaseMs": 5, "ops": [
                {"invoke": "t", "args": {"a": 1}, "callId": "c", "timeoutMs": 9, "cancelAfterMs": 3, "idempotencyKey": "k"},
                {"read": "r"}, {"catalog": 100}, {"delay": 7}, {"awaitSleep": true}, {"wake": true}
            ]}}),
        };
        let args = case.host_args().unwrap();
        assert_eq!(
            args.join(" "),
            "--tool-info --trace --lease-ms 5 --invoke t --args {\"a\":1} --call-id c --idempotency-key k --invoke-timeout-ms 9 \
             --cancel-after-ms 3 --read r --catalog 100 --delay 7 --await-sleep --wake"
        );
        assert!(op_args(&json!({"bogus": 1})).is_err());
    }
}
