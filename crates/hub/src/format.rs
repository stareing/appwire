//! 工具格式导出与分派（spec/hub-api.md 第 5 节）。
//!
//! - 导出：[`HubTool`] → 各 LLM 厂商的工具定义格式。
//! - 分派：模型返回的工具调用 → 全名 + 参数；执行结果（MCP `CallToolResult`）→ 该格式的“工具结果”消息。
//! - 名称编码：见 [`NameCodec`]。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::str::FromStr;

use app_mcp_protocol::Risk;
use rmcp::model::CallToolResult;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::call::{UNAVAILABLE_PREFIX, result_text};
use crate::types::{Availability, HubTool, risk_str};

/// OpenAI / Anthropic 工具名长度上限。
pub const MAX_NAME_LEN: usize = 64;

/// 哈希后缀长度（`_` + 4 位十六进制）。
const HASH_SUFFIX_LEN: usize = 5;

/// 工具定义 / 调用格式。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ToolFormat {
    /// MCP `tools/list` 条目 / `tools/call` 参数与结果。
    Mcp,
    /// OpenAI Chat Completions（`tools` / `tool_calls` / `role: tool`）。
    OpenAiChat,
    /// OpenAI Responses API（`function_call` / `function_call_output`）。
    OpenAiResponses,
    /// Anthropic Messages（`tool_use` / `tool_result`）。
    Anthropic,
    /// Google Gemini（`functionDeclarations` / `functionCall` / `functionResponse`）。
    Gemini,
}

impl FromStr for ToolFormat {
    type Err = String;

    /// 接受 `mcp`、`openai-chat`（或 `openai`）、`openai-responses`、`anthropic`、`gemini`，
    /// 不区分大小写，`-` / `_` 可省略。
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let norm: String = s
            .chars()
            .filter(|c| !matches!(c, '-' | '_' | ' '))
            .collect::<String>()
            .to_ascii_lowercase();
        Ok(match norm.as_str() {
            "mcp" => ToolFormat::Mcp,
            "openai" | "openaichat" | "chat" => ToolFormat::OpenAiChat,
            "openairesponses" | "responses" => ToolFormat::OpenAiResponses,
            "anthropic" | "claude" => ToolFormat::Anthropic,
            "gemini" | "google" => ToolFormat::Gemini,
            _ => return Err(format!("未知的工具格式：{s}")),
        })
    }
}

// ---------------------------------------------------------------------------
// 名称编码
// ---------------------------------------------------------------------------

/// 全名 ↔ 导出名的双向映射。
///
/// 规则：导出名 = 全名中的 `.` 换成 `__`（其他不在 `[a-zA-Z0-9_-]` 内的字符换成 `_`，
/// 此时视为有损，必加哈希）。若与其他工具的导出名冲突、超过 64 字符或有损，
/// 则截断到 59 字符并追加 `_<sha256(全名) 前 4 位十六进制>`；哈希后仍冲突时对 `全名#n` 取哈希重试。
/// 结果只取决于全名集合（与顺序无关），同一集合总是得到同一映射。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NameCodec {
    /// 全名 → 导出名。
    to_export: BTreeMap<String, String>,
    /// 导出名 → 全名。
    to_full: HashMap<String, String>,
}

fn sanitize(full: &str) -> (String, bool) {
    let mut out = String::with_capacity(full.len() + 4);
    let mut lossy = false;
    for c in full.chars() {
        match c {
            '.' => out.push_str("__"),
            c if c.is_ascii_alphanumeric() || c == '_' || c == '-' => out.push(c),
            _ => {
                out.push('_');
                lossy = true;
            }
        }
    }
    let lossy = lossy || out.is_empty();
    (out, lossy)
}

fn hash4(full: &str, salt: u32) -> String {
    let mut h = Sha256::new();
    h.update(full.as_bytes());
    if salt > 0 {
        h.update(format!("#{salt}").as_bytes());
    }
    let d = h.finalize();
    format!("{:02x}{:02x}", d[0], d[1])
}

impl NameCodec {
    pub fn new<'a>(full_names: impl IntoIterator<Item = &'a str>) -> Self {
        let mut names: Vec<&str> = full_names.into_iter().collect();
        names.sort_unstable();
        names.dedup();
        let bases: Vec<(String, bool)> = names.iter().map(|n| sanitize(n)).collect();
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for (b, _) in &bases {
            *counts.entry(b.as_str()).or_default() += 1;
        }
        let mut codec = NameCodec::default();
        let mut used: HashSet<String> = HashSet::new();
        let mut hashed = Vec::new();
        for (full, (base, lossy)) in names.iter().zip(&bases) {
            if !lossy && base.len() <= MAX_NAME_LEN && counts[base.as_str()] == 1 {
                used.insert(base.clone());
                codec.insert(full, base.clone());
            } else {
                hashed.push((*full, base.as_str()));
            }
        }
        for (full, base) in hashed {
            // base 只含 ASCII，按字节截断安全。
            let prefix = &base[..base.len().min(MAX_NAME_LEN - HASH_SUFFIX_LEN)];
            let mut salt = 0;
            let name = loop {
                let cand = format!("{prefix}_{}", hash4(full, salt));
                if !used.contains(&cand) {
                    break cand;
                }
                salt += 1;
            };
            used.insert(name.clone());
            codec.insert(full, name);
        }
        codec
    }

    fn insert(&mut self, full: &str, export: String) {
        self.to_full.insert(export.clone(), full.to_owned());
        self.to_export.insert(full.to_owned(), export);
    }

    pub fn export_name(&self, full: &str) -> Option<&str> {
        self.to_export.get(full).map(String::as_str)
    }

    pub fn full_name(&self, export: &str) -> Option<&str> {
        self.to_full.get(export).map(String::as_str)
    }

    /// `(全名, 导出名)`，按全名排序。
    pub fn pairs(&self) -> impl Iterator<Item = (&str, &str)> {
        self.to_export.iter().map(|(f, e)| (f.as_str(), e.as_str()))
    }
}

// ---------------------------------------------------------------------------
// 导出
// ---------------------------------------------------------------------------

/// 导出用的描述：不可用前缀 + 原描述 + 风险标记（risk 不是 read / write 时）。
pub fn export_description(t: &HubTool) -> String {
    let mut d = match t.availability {
        Availability::NotRegistered => format!("{UNAVAILABLE_PREFIX}{}", t.description),
        _ => t.description.clone(),
    };
    if !matches!(t.risk, Risk::Read | Risk::Write) {
        d.push_str(&format!("（风险：{}）", risk_str(t.risk)));
    }
    d
}

/// 原样 JSON Schema，只去掉顶层 `$schema`；不是对象时用 `{"type":"object"}`。
fn plain_schema(t: &HubTool) -> Value {
    match &t.input_schema {
        Value::Object(m) => {
            let mut m = m.clone();
            m.remove("$schema");
            Value::Object(m)
        }
        _ => json!({ "type": "object" }),
    }
}

/// Gemini 不支持的 schema 关键字。
const GEMINI_DROP: &[&str] = &[
    "$schema",
    "$id",
    "$ref",
    "$defs",
    "$comment",
    "definitions",
    "additionalProperties",
    "patternProperties",
    "unevaluatedProperties",
    "propertyNames",
    "dependentRequired",
    "dependentSchemas",
    "if",
    "then",
    "else",
    "not",
    "examples",
    "readOnly",
    "writeOnly",
    "contentEncoding",
    "contentMediaType",
];

/// 把 JSON Schema 适配为 Gemini 支持的子集（只处理 schema 位置，不误删同名属性）：
/// 剔除 [`GEMINI_DROP`]；`oneOf` → `anyOf`；`const` → `enum`；
/// `type: [T, "null"]` → `type: T, nullable: true`。
pub fn gemini_schema(v: &Value) -> Value {
    let Value::Object(m) = v else {
        return v.clone();
    };
    let mut out = Map::new();
    for (k, val) in m {
        if GEMINI_DROP.contains(&k.as_str()) {
            continue;
        }
        match k.as_str() {
            "properties" => {
                let props = match val {
                    Value::Object(p) => Value::Object(
                        p.iter()
                            .map(|(name, s)| (name.clone(), gemini_schema(s)))
                            .collect(),
                    ),
                    other => other.clone(),
                };
                out.insert(k.clone(), props);
            }
            "items" => {
                let items = match val {
                    Value::Array(a) => a.first().map(gemini_schema).unwrap_or(json!({})),
                    other => gemini_schema(other),
                };
                out.insert(k.clone(), items);
            }
            "anyOf" | "oneOf" | "allOf" => {
                if let Value::Array(a) = val {
                    let key = if k == "allOf" { "allOf" } else { "anyOf" };
                    out.insert(
                        key.to_owned(),
                        Value::Array(a.iter().map(gemini_schema).collect()),
                    );
                }
            }
            "const" => {
                out.insert("enum".to_owned(), json!([val]));
            }
            "type" => match val {
                Value::Array(types) => {
                    let non_null: Vec<&Value> =
                        types.iter().filter(|t| t.as_str() != Some("null")).collect();
                    if non_null.len() < types.len() {
                        out.insert("nullable".to_owned(), json!(true));
                    }
                    if let Some(t) = non_null.first() {
                        out.insert("type".to_owned(), (*t).clone());
                    }
                }
                other => {
                    out.insert(k.clone(), other.clone());
                }
            },
            _ => {
                out.insert(k.clone(), val.clone());
            }
        }
    }
    Value::Object(out)
}

fn export_one(format: ToolFormat, t: &HubTool, name: &str) -> Value {
    let description = export_description(t);
    match format {
        ToolFormat::Mcp => {
            let mut v = json!({
                "name": name,
                "description": description,
                "inputSchema": plain_schema(t),
                "annotations": t.annotations,
            });
            if let Some(title) = &t.title {
                v["title"] = json!(title);
            }
            if let Some(output) = &t.output_schema {
                v["outputSchema"] = Value::Object(crate::mcp_convert::mcp_output_schema(output));
            }
            v
        }
        ToolFormat::OpenAiChat => json!({
            "type": "function",
            "function": { "name": name, "description": description, "parameters": plain_schema(t) },
        }),
        ToolFormat::OpenAiResponses => json!({
            "type": "function",
            "name": name,
            "description": description,
            "parameters": plain_schema(t),
        }),
        ToolFormat::Anthropic => json!({
            "name": name,
            "description": description,
            "input_schema": plain_schema(t),
        }),
        ToolFormat::Gemini => {
            let mut v = json!({ "name": name, "description": description });
            let params = gemini_schema(&plain_schema(t));
            // 无参数的函数不带 parameters（Gemini 不接受空的 properties）。
            let has_props = params
                .get("properties")
                .and_then(Value::as_object)
                .is_some_and(|p| !p.is_empty());
            if has_props {
                v["parameters"] = params;
            }
            v
        }
    }
}

/// 导出工具列表（名称按 `codec` 编码；codec 中没有的工具现场编码）。
pub fn export(format: ToolFormat, tools: &[HubTool], codec: &NameCodec) -> Value {
    let items: Vec<Value> = tools
        .iter()
        .map(|t| {
            let name = codec
                .export_name(&t.name)
                .map(str::to_owned)
                .unwrap_or_else(|| sanitize(&t.name).0);
            export_one(format, t, &name)
        })
        .collect();
    match format {
        ToolFormat::Gemini => json!({ "functionDeclarations": items }),
        _ => Value::Array(items),
    }
}

// ---------------------------------------------------------------------------
// 分派
// ---------------------------------------------------------------------------

/// 解析后的工具调用。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ParsedCall {
    /// 调用 ID（OpenAI `id` / `call_id`、Anthropic `id`、Gemini `id`）；同时作为 callId，可被 `cancel_call` 取消。
    pub id: Option<String>,
    /// 模型给出的名称（导出名或全名）。
    pub name: String,
    pub arguments: Value,
    /// 参数无法解析时的说明（仍需以该格式返回错误结果）。
    pub error: Option<String>,
}

fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_owned)
}

/// 参数：对象原样；JSON 字符串解析；缺省 / 空串 / `null` 为 `{}`。
fn parse_arguments(v: Option<&Value>) -> Result<Value, String> {
    match v {
        None | Some(Value::Null) => Ok(json!({})),
        Some(Value::Object(_)) => Ok(v.cloned().unwrap_or_default()),
        Some(Value::String(s)) if s.trim().is_empty() => Ok(json!({})),
        Some(Value::String(s)) => match serde_json::from_str::<Value>(s) {
            Ok(v @ Value::Object(_)) => Ok(v),
            Ok(_) => Err("工具参数必须是 JSON 对象。".to_owned()),
            Err(e) => Err(format!("工具参数不是合法的 JSON：{e}")),
        },
        Some(_) => Err("工具参数必须是 JSON 对象。".to_owned()),
    }
}

/// 解析模型返回的工具调用。缺少名称时返回 `Err`（仍带尽量解析出的 ID，用于构造错误结果）。
pub fn parse_call(format: ToolFormat, v: &Value) -> Result<ParsedCall, ParsedCall> {
    let (id, name, args) = match format {
        ToolFormat::Mcp => {
            // 也接受完整的 JSON-RPC `tools/call` 请求。
            let p = v.get("params").filter(|p| p.is_object()).unwrap_or(v);
            (None, str_field(p, "name"), p.get("arguments"))
        }
        ToolFormat::OpenAiChat => {
            let f = v.get("function").unwrap_or(v);
            (str_field(v, "id"), str_field(f, "name"), f.get("arguments"))
        }
        ToolFormat::OpenAiResponses => (
            str_field(v, "call_id").or_else(|| str_field(v, "id")),
            str_field(v, "name"),
            v.get("arguments"),
        ),
        ToolFormat::Anthropic => (str_field(v, "id"), str_field(v, "name"), v.get("input")),
        ToolFormat::Gemini => {
            let c = v.get("functionCall").unwrap_or(v);
            (str_field(c, "id"), str_field(c, "name"), c.get("args"))
        }
    };
    let (arguments, error) = match parse_arguments(args) {
        Ok(a) => (a, None),
        Err(e) => (json!({}), Some(e)),
    };
    match name.filter(|n| !n.is_empty()) {
        Some(name) => Ok(ParsedCall {
            id,
            name,
            arguments,
            error,
        }),
        None => Err(ParsedCall {
            id,
            name: String::new(),
            arguments,
            error: Some("工具调用缺少名称（name）。".to_owned()),
        }),
    }
}

/// 把执行结果包装为该格式的“工具结果”消息。
pub fn render_result(format: ToolFormat, call: &ParsedCall, r: &CallToolResult) -> Value {
    let text = result_text(r);
    let is_error = r.is_error == Some(true);
    match format {
        ToolFormat::Mcp => serde_json::to_value(r).unwrap_or(Value::Null),
        ToolFormat::OpenAiChat => json!({
            "role": "tool",
            "tool_call_id": call.id,
            "content": text,
        }),
        ToolFormat::OpenAiResponses => json!({
            "type": "function_call_output",
            "call_id": call.id,
            "output": text,
        }),
        ToolFormat::Anthropic => {
            let mut v = json!({
                "type": "tool_result",
                "tool_use_id": call.id,
                "content": text,
            });
            if is_error {
                v["is_error"] = json!(true);
            }
            v
        }
        ToolFormat::Gemini => {
            let response = if is_error {
                json!({ "error": text })
            } else {
                json!({ "output": text })
            };
            let mut fr = json!({ "name": call.name, "response": response });
            if let Some(id) = &call.id {
                fr["id"] = json!(id);
            }
            json!({ "functionResponse": fr })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use app_mcp_protocol::Activation;
    use rmcp::model::ContentBlock;

    fn tool(name: &str, risk: Risk, schema: Value) -> HubTool {
        let (app, t) = name.split_once('.').unwrap();
        HubTool {
            name: name.into(),
            app_id: app.into(),
            tool: t.into(),
            title: None,
            description: "描述".into(),
            input_schema: schema,
            risk,
            activation: Activation::Foreground,
            availability: Availability::Available,
            annotations: risk.annotations(),
            output_schema: None,
        }
    }

    fn valid(n: &str) -> bool {
        !n.is_empty()
            && n.len() <= MAX_NAME_LEN
            && n.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    }

    #[test]
    fn simple_names() {
        let c = NameCodec::new(["shop.cart.add", "apps.list", "files.read_file"]);
        assert_eq!(c.export_name("shop.cart.add"), Some("shop__cart__add"));
        assert_eq!(c.export_name("apps.list"), Some("apps__list"));
        assert_eq!(c.export_name("files.read_file"), Some("files__read_file"));
        assert_eq!(c.full_name("shop__cart__add"), Some("shop.cart.add"));
        assert_eq!(c.full_name("nope"), None);
    }

    #[test]
    fn conflicts_get_hash_suffix() {
        // "shop.x.y" 与 "shop.x__y" 都编码为 "shop__x__y"。
        let c = NameCodec::new(["shop.x.y", "shop.x__y", "shop.z"]);
        let a = c.export_name("shop.x.y").unwrap().to_owned();
        let b = c.export_name("shop.x__y").unwrap().to_owned();
        assert_ne!(a, b);
        for n in [&a, &b] {
            assert!(n.starts_with("shop__x__y_"), "{n}");
            assert_eq!(n.len(), "shop__x__y_".len() + 4);
            assert!(valid(n));
        }
        assert_eq!(c.export_name("shop.z"), Some("shop__z"));
        assert_eq!(c.full_name(&a), Some("shop.x.y"));
        assert_eq!(c.full_name(&b), Some("shop.x__y"));
        // 与顺序无关
        let c2 = NameCodec::new(["shop.z", "shop.x__y", "shop.x.y"]);
        assert_eq!(c, c2);
    }

    #[test]
    fn long_and_lossy_names() {
        let long = format!("app.{}", "a".repeat(70));
        let c = NameCodec::new([long.as_str(), "up.weird/name"]);
        let e = c.export_name(&long).unwrap();
        assert_eq!(e.len(), MAX_NAME_LEN);
        assert!(valid(e));
        assert_eq!(c.full_name(e), Some(long.as_str()));
        let w = c.export_name("up.weird/name").unwrap();
        assert!(w.starts_with("up__weird_name_"), "{w}");
        assert!(valid(w));
        // 两个超长名称前缀相同，也不冲突
        let l1 = format!("app.{}1", "b".repeat(70));
        let l2 = format!("app.{}2", "b".repeat(70));
        let c = NameCodec::new([l1.as_str(), l2.as_str()]);
        assert_ne!(c.export_name(&l1), c.export_name(&l2));
    }

    #[test]
    fn hashed_name_avoids_plain_name() {
        // 构造：某个普通名称恰好等于另一个工具的哈希候选名。
        let base = "shop__x__y";
        let cand = format!("{base}_{}", hash4("shop.x.y", 0));
        let plain_full = cand.replacen("shop__", "shop.", 1);
        let c = NameCodec::new(["shop.x.y", "shop.x__y", plain_full.as_str()]);
        let names: HashSet<&str> = c.pairs().map(|(_, e)| e).collect();
        assert_eq!(names.len(), 3);
    }

    #[test]
    fn descriptions_and_formats() {
        let t = tool(
            "shop.cart.checkout",
            Risk::Payment,
            json!({"$schema": "x", "type": "object", "properties": {"n": {"type": "integer"}}}),
        );
        assert_eq!(export_description(&t), "描述（风险：payment）");
        let codec = NameCodec::new([t.name.as_str()]);
        let ts = std::slice::from_ref(&t);

        let v = export(ToolFormat::Anthropic, ts, &codec);
        assert_eq!(v[0]["name"], "shop__cart__checkout");
        assert_eq!(v[0]["input_schema"]["type"], "object");
        assert!(v[0]["input_schema"].get("$schema").is_none());

        let v = export(ToolFormat::OpenAiChat, ts, &codec);
        assert_eq!(v[0]["type"], "function");
        assert_eq!(v[0]["function"]["name"], "shop__cart__checkout");
        assert_eq!(v[0]["function"]["parameters"]["properties"]["n"]["type"], "integer");

        let v = export(ToolFormat::OpenAiResponses, ts, &codec);
        assert_eq!(v[0]["name"], "shop__cart__checkout");
        assert!(v[0]["parameters"].is_object());

        let v = export(ToolFormat::Mcp, ts, &codec);
        assert_eq!(v[0]["inputSchema"]["type"], "object");
        assert_eq!(v[0]["annotations"]["destructiveHint"], true);

        let v = export(ToolFormat::Gemini, ts, &codec);
        assert_eq!(v["functionDeclarations"][0]["name"], "shop__cart__checkout");

        let read = tool("a.b", Risk::Read, json!({"type": "object", "properties": {}}));
        assert_eq!(export_description(&read), "描述");
        let v = export(ToolFormat::Gemini, std::slice::from_ref(&read), &NameCodec::default());
        assert_eq!(v["functionDeclarations"][0]["name"], "a__b");
        assert!(v["functionDeclarations"][0].get("parameters").is_none());
    }

    #[test]
    fn gemini_schema_strips_unsupported() {
        let s = json!({
            "type": "object",
            "additionalProperties": false,
            "$defs": {"x": {}},
            "properties": {
                "if": {"type": ["string", "null"], "$ref": "#/x"},
                "mode": {"const": "fast"},
                "list": {"type": "array", "items": {"type": "object", "additionalProperties": true}},
                "u": {"oneOf": [{"type": "string"}, {"type": "integer"}]}
            },
            "required": ["if"]
        });
        let g = gemini_schema(&s);
        assert_eq!(
            g,
            json!({
                "type": "object",
                "properties": {
                    "if": {"type": "string", "nullable": true},
                    "mode": {"enum": ["fast"]},
                    "list": {"type": "array", "items": {"type": "object"}},
                    "u": {"anyOf": [{"type": "string"}, {"type": "integer"}]}
                },
                "required": ["if"]
            })
        );
    }

    #[test]
    fn parse_calls() {
        let p = parse_call(
            ToolFormat::OpenAiChat,
            &json!({"id": "c1", "type": "function", "function": {"name": "a__b", "arguments": "{\"x\":1}"}}),
        )
        .unwrap();
        assert_eq!((p.id.as_deref(), p.name.as_str()), (Some("c1"), "a__b"));
        assert_eq!(p.arguments, json!({"x": 1}));
        let p = parse_call(
            ToolFormat::OpenAiChat,
            &json!({"id": "c1", "function": {"name": "a__b", "arguments": "{oops"}}),
        )
        .unwrap();
        assert!(p.error.unwrap().contains("JSON"));
        let p = parse_call(
            ToolFormat::OpenAiResponses,
            &json!({"type": "function_call", "call_id": "r1", "name": "a__b", "arguments": ""}),
        )
        .unwrap();
        assert_eq!(p.id.as_deref(), Some("r1"));
        assert_eq!(p.arguments, json!({}));
        let p = parse_call(
            ToolFormat::Anthropic,
            &json!({"type": "tool_use", "id": "t1", "name": "a__b", "input": {"y": 2}}),
        )
        .unwrap();
        assert_eq!(p.arguments, json!({"y": 2}));
        let p = parse_call(
            ToolFormat::Gemini,
            &json!({"functionCall": {"name": "a__b", "args": {"z": 3}}}),
        )
        .unwrap();
        assert_eq!((p.id, p.arguments), (None, json!({"z": 3})));
        let p = parse_call(ToolFormat::Gemini, &json!({"name": "a__b"})).unwrap();
        assert_eq!(p.arguments, json!({}));
        let p = parse_call(
            ToolFormat::Mcp,
            &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "a.b", "arguments": {"k": 1}}}),
        )
        .unwrap();
        assert_eq!((p.name.as_str(), p.arguments), ("a.b", json!({"k": 1})));
        let e = parse_call(ToolFormat::Anthropic, &json!({"id": "t9", "input": {}})).unwrap_err();
        assert_eq!(e.id.as_deref(), Some("t9"));
        assert!(e.error.is_some());
        assert!(
            parse_call(ToolFormat::Anthropic, &json!({"name": "x", "input": [1]}))
                .unwrap()
                .error
                .is_some()
        );
    }

    #[test]
    fn render_results() {
        let call = ParsedCall {
            id: Some("x1".into()),
            name: "a__b".into(),
            arguments: json!({}),
            error: None,
        };
        let ok = CallToolResult::success(vec![ContentBlock::text("{\"a\":1}")]);
        let err = CallToolResult::error(vec![ContentBlock::text("USER_REJECTED: 不")]);
        assert_eq!(
            render_result(ToolFormat::OpenAiChat, &call, &ok),
            json!({"role": "tool", "tool_call_id": "x1", "content": "{\"a\":1}"})
        );
        assert_eq!(
            render_result(ToolFormat::OpenAiResponses, &call, &err),
            json!({"type": "function_call_output", "call_id": "x1", "output": "USER_REJECTED: 不"})
        );
        assert_eq!(
            render_result(ToolFormat::Anthropic, &call, &ok),
            json!({"type": "tool_result", "tool_use_id": "x1", "content": "{\"a\":1}"})
        );
        assert_eq!(
            render_result(ToolFormat::Anthropic, &call, &err)["is_error"],
            true
        );
        assert_eq!(
            render_result(ToolFormat::Gemini, &call, &err),
            json!({"functionResponse": {"name": "a__b", "id": "x1", "response": {"error": "USER_REJECTED: 不"}}})
        );
        let m = render_result(ToolFormat::Mcp, &call, &err);
        assert_eq!(m["isError"], true);
    }

    #[test]
    fn format_from_str() {
        assert_eq!("openai-chat".parse(), Ok(ToolFormat::OpenAiChat));
        assert_eq!("OpenAI_Responses".parse(), Ok(ToolFormat::OpenAiResponses));
        assert_eq!("anthropic".parse(), Ok(ToolFormat::Anthropic));
        assert_eq!("gemini".parse(), Ok(ToolFormat::Gemini));
        assert_eq!("MCP".parse(), Ok(ToolFormat::Mcp));
        assert!("x".parse::<ToolFormat>().is_err());
        assert_eq!(
            serde_json::to_value(ToolFormat::OpenAiChat).unwrap(),
            json!("openAiChat")
        );
    }
}
