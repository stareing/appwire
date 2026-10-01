//! JSON-RPC 2.0 信封。
//!
//! 只负责区分请求 / 通知 / 响应并保留原始 `params` / `result`，
//! 具体参数类型由调用方按方法名用 [`serde_json::from_value`] 解析。

use serde::de::{self, Deserializer, Unexpected, Visitor};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::{Map, Value, json};

/// 请求 ID：JSON-RPC 允许数字或字符串。
///
/// @why 序列化 / 反序列化手写而不用 `#[serde(untagged)]`：untagged 会链接 serde 的 `Content`
/// 缓冲反序列化（WASM 中约数 KB）；语义相同——数字须在 i64 范围内，其余类型报错。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum RequestId {
    Number(i64),
    String(String),
}

impl Serialize for RequestId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            RequestId::Number(n) => serializer.serialize_i64(*n),
            RequestId::String(s) => serializer.serialize_str(s),
        }
    }
}

impl<'de> Deserialize<'de> for RequestId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(RequestIdVisitor)
    }
}

struct RequestIdVisitor;

impl Visitor<'_> for RequestIdVisitor {
    type Value = RequestId;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a number (i64) or string request id")
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<RequestId, E> {
        Ok(RequestId::Number(v))
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<RequestId, E> {
        i64::try_from(v).map(RequestId::Number).map_err(|_| E::invalid_value(Unexpected::Unsigned(v), &self))
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<RequestId, E> {
        Ok(RequestId::String(v.to_owned()))
    }

    fn visit_string<E: de::Error>(self, v: String) -> Result<RequestId, E> {
        Ok(RequestId::String(v))
    }
}

impl RequestId {
    /// @why 直接构造而不经 `serde_json::to_value(..).expect(..)`：免去不可能失败的错误分支。
    fn to_value(&self) -> Value {
        match self {
            RequestId::Number(n) => Value::from(*n),
            RequestId::String(s) => Value::String(s.clone()),
        }
    }
}

impl From<i64> for RequestId {
    fn from(v: i64) -> Self {
        RequestId::Number(v)
    }
}

impl From<String> for RequestId {
    fn from(v: String) -> Self {
        RequestId::String(v)
    }
}

impl From<&str> for RequestId {
    fn from(v: &str) -> Self {
        RequestId::String(v.to_owned())
    }
}

impl std::fmt::Display for RequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RequestId::Number(n) => write!(f, "{n}"),
            RequestId::String(s) => f.write_str(s),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub id: RequestId,
    pub method: String,
    /// 缺省时为 `Value::Null`。
    pub params: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Notification {
    pub method: String,
    /// 缺省时为 `Value::Null`。
    pub params: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Response {
    pub id: RequestId,
    pub outcome: Result<Value, RpcError>,
}

/// JSON-RPC 错误对象。协议层错误的类别放在 `data.kind` 中（见 [`crate::ErrorKind`]）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, thiserror::Error)]
#[error("rpc error {code}: {message}")]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl RpcError {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL_ERROR: i64 = -32603;

    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), data: None }
    }

    pub fn method_not_found(method: &str) -> Self {
        Self::new(Self::METHOD_NOT_FOUND, format!("method not found: {method}"))
    }

    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(Self::INVALID_PARAMS, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(Self::INTERNAL_ERROR, message)
    }
}

/// 一条完整的 JSON-RPC 消息。
#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    Request(Request),
    Notification(Notification),
    Response(Response),
}

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("invalid json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid json-rpc message: {0}")]
    Invalid(&'static str),
}

impl Message {
    pub fn request(id: impl Into<RequestId>, method: impl Into<String>, params: Value) -> Self {
        Message::Request(Request { id: id.into(), method: method.into(), params })
    }

    pub fn notification(method: impl Into<String>, params: Value) -> Self {
        Message::Notification(Notification { method: method.into(), params })
    }

    pub fn result(id: RequestId, result: Value) -> Self {
        Message::Response(Response { id, outcome: Ok(result) })
    }

    pub fn error(id: RequestId, error: RpcError) -> Self {
        Message::Response(Response { id, outcome: Err(error) })
    }

    /// 解析一条文本消息。批量消息（数组）不在协议范围内，返回错误。
    pub fn parse(text: &str) -> Result<Self, ParseError> {
        let value: Value = serde_json::from_str(text)?;
        Self::from_value(value)
    }

    pub fn from_value(value: Value) -> Result<Self, ParseError> {
        let Value::Object(mut obj) = value else {
            return Err(ParseError::Invalid("message must be a json object"));
        };
        if obj.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return Err(ParseError::Invalid("missing or wrong \"jsonrpc\" field"));
        }
        let id = match obj.remove("id") {
            None | Some(Value::Null) => None,
            Some(v) => Some(
                serde_json::from_value::<RequestId>(v)
                    .map_err(|_| ParseError::Invalid("id must be a number or string"))?,
            ),
        };
        let params = obj.remove("params").unwrap_or(Value::Null);

        if let Some(method) = obj.remove("method") {
            let Value::String(method) = method else {
                return Err(ParseError::Invalid("method must be a string"));
            };
            return Ok(match id {
                Some(id) => Message::Request(Request { id, method, params }),
                None => Message::Notification(Notification { method, params }),
            });
        }

        let Some(id) = id else {
            return Err(ParseError::Invalid("response without id"));
        };
        if let Some(err) = obj.remove("error") {
            let err: RpcError = serde_json::from_value(err)
                .map_err(|_| ParseError::Invalid("malformed error object"))?;
            return Ok(Message::Response(Response { id, outcome: Err(err) }));
        }
        match obj.remove("result") {
            Some(result) => Ok(Message::Response(Response { id, outcome: Ok(result) })),
            None => Err(ParseError::Invalid("response has neither result nor error")),
        }
    }

    pub fn to_value(&self) -> Value {
        let mut obj = Map::new();
        obj.insert("jsonrpc".into(), json!("2.0"));
        match self {
            Message::Request(r) => {
                obj.insert("id".into(), r.id.to_value());
                obj.insert("method".into(), Value::String(r.method.clone()));
                if !r.params.is_null() {
                    obj.insert("params".into(), r.params.clone());
                }
            }
            Message::Notification(n) => {
                obj.insert("method".into(), Value::String(n.method.clone()));
                if !n.params.is_null() {
                    obj.insert("params".into(), n.params.clone());
                }
            }
            Message::Response(r) => {
                obj.insert("id".into(), r.id.to_value());
                match &r.outcome {
                    Ok(result) => {
                        obj.insert("result".into(), result.clone());
                    }
                    Err(err) => {
                        obj.insert("error".into(), serde_json::to_value(err).expect("error serializes"));
                    }
                }
            }
        }
        Value::Object(obj)
    }

    pub fn to_json(&self) -> String {
        self.to_value().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_all_kinds() {
        let msgs = [
            Message::request(1, "app/hello", json!({"appId": "shop"})),
            Message::request("abc", "ping", Value::Null),
            Message::notification("tools/sync", json!({"tools": []})),
            Message::result(RequestId::Number(7), json!({})),
            Message::error(RequestId::from("x"), RpcError::method_not_found("nope")),
        ];
        for m in msgs {
            assert_eq!(Message::parse(&m.to_json()).unwrap(), m);
        }
    }

    #[test]
    fn rejects_bad_messages() {
        assert!(Message::parse("[]").is_err());
        assert!(Message::parse(r#"{"id":1,"method":"x"}"#).is_err());
        assert!(Message::parse(r#"{"jsonrpc":"2.0","id":1}"#).is_err());
        assert!(Message::parse(r#"{"jsonrpc":"2.0","method":5}"#).is_err());
        assert!(Message::parse("not json").is_err());
    }

    #[test]
    fn request_id_accepts_i64_and_string_only() {
        for (text, want) in [
            ("7", Some(RequestId::Number(7))),
            ("-3", Some(RequestId::Number(-3))),
            (r#""abc""#, Some(RequestId::from("abc"))),
            ("9223372036854775808", None),
            ("1.5", None),
            ("null", None),
            ("[1]", None),
            ("{}", None),
        ] {
            assert_eq!(serde_json::from_str::<RequestId>(text).ok(), want, "{text}");
            let value: Value = serde_json::from_str(text).unwrap();
            assert_eq!(serde_json::from_value::<RequestId>(value).ok(), want, "{text}");
        }
        assert_eq!(serde_json::to_string(&RequestId::Number(-3)).unwrap(), "-3");
        assert_eq!(serde_json::to_string(&RequestId::from("a")).unwrap(), r#""a""#);
        assert_eq!(RequestId::Number(5).to_value(), json!(5));
        assert_eq!(RequestId::from("x").to_value(), json!("x"));
    }

    #[test]
    fn null_result_is_a_result() {
        let m = Message::parse(r#"{"jsonrpc":"2.0","id":3,"result":null}"#).unwrap();
        assert_eq!(m, Message::result(RequestId::Number(3), Value::Null));
    }
}
