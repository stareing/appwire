//! JS 对象 ↔ 核心类型的转换。与 wasm-bindgen 无关，可在原生目标上测试。
//!
//! 输入（配置、定义、结果）由 [`FromJson`] 从 JSON 值手工读取：serde 的 `derive(Deserialize)` 会为每个结构
//! 各生成一套对象 / 数组两种形式的反序列化代码，是 WASM 体积的主要来源之一。协议类型（`AppOverview`、
//! `WakeDescriptor`、各枚举）仍用其 serde 定义，保持单一来源。输出（状态、事件）用 serde 序列化。

use app_mcp_core::{
    BusyPolicy, CallDedupPolicy, CallOutput, CancelReason, ClientConfig, ClientKind, ConnectionState, Event, HeartbeatMode,
    HeartbeatPolicy, LifecycleMode, LifecyclePolicy, ReconnectPolicy, Residency, ResourceDef, ScopeId, SleepReason, ToolDef, ToolError,
    ToolSurface, ToolUpdate, TransportKind, Visibility, WakeReason,
};
use app_mcp_core::{
    Activation, AppOverview, Audience, ContentAnnotations, ResultStatus, Risk, ToolAnnotations, WakeDescriptor,
};
use app_mcp_protocol::ErrorKind;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

mod config;
mod definitions;
mod outcome;
#[cfg(test)]
mod tests;

pub use config::*;
pub use definitions::*;
pub use outcome::*;

/// JS 能精确表示的最大整数（2^53 - 1）。
pub const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;

/// JS 数字句柄 → u64。非整数、负数或超出安全整数范围视为无效。
///
/// @error `无效的<what>：<数值>`。
/// @why 数值经 serde_json 的 `Number` 格式化（复用已链接的 zmij），不引入 `f64` 的 `Display`（体积）。
pub fn handle(id: f64, what: &str) -> Result<u64, String> {
    if id.is_finite() && id >= 0.0 && id.fract() == 0.0 && id <= MAX_SAFE_INTEGER as f64 {
        return Ok(id as u64);
    }
    let shown = match serde_json::Number::from_f64(id) {
        Some(n) => n.to_string(),
        None if id.is_nan() => "NaN".to_owned(),
        None if id > 0.0 => "Infinity".to_owned(),
        None => "-Infinity".to_owned(),
    };
    Err(format!("无效的{what}：{shown}"))
}

fn scope_handle(scope: Option<f64>) -> Result<Option<ScopeId>, String> {
    scope.map(|s| handle(s, " scope 句柄").map(ScopeId)).transpose()
}

pub fn parse_wake_reason(s: &str) -> Result<WakeReason, String> {
    match s {
        "os-activation" => Ok(WakeReason::OsActivation),
        "app" => Ok(WakeReason::App),
        "visible" => Ok(WakeReason::Visible),
        "cold-start" => Ok(WakeReason::ColdStart),
        other => Err(format!("无效的唤醒原因：\"{other}\"")),
    }
}

pub fn parse_sleep_reason(s: &str) -> Result<SleepReason, String> {
    match s {
        "idle" => Ok(SleepReason::Idle),
        "grace" => Ok(SleepReason::Grace),
        "background" => Ok(SleepReason::Background),
        "app" => Ok(SleepReason::App),
        other => Err(format!("无效的休眠原因：\"{other}\"")),
    }
}

pub fn parse_visibility(s: &str) -> Result<Visibility, String> {
    match s {
        "visible" => Ok(Visibility::Visible),
        "hidden" => Ok(Visibility::Hidden),
        "frozen" => Ok(Visibility::Frozen),
        other => Err(format!("无效的可见性：\"{other}\"")),
    }
}

// ---------------------------------------------------------------------------
// JSON 输入读取
// ---------------------------------------------------------------------------

/// 从 JS 传入的 JSON 值构造。
///
/// @error 返回中文说明（不含"格式错误"前缀，由调用方加上所在参数名）。
pub trait FromJson: Sized {
    fn from_json(value: Value) -> Result<Self, String>;
}

/// JSON 对象的字段读取器：逐个取走字段；`null` 与缺省等同（与 serde 的 `Option` 一致），未知字段忽略。
/// 读取失败时记下第一条错误并返回缺省值，读完后由 [`Fields::finish`] 统一返回。
///
/// @error 类型不符：`字段 <key> 应为<类型>`；必填字段缺失：`缺少字段 <key>`；嵌套对象的错误前加 `<key>.`。
/// @why 不在每个字段上 `?` 提前返回：每个提前返回点都要生成一份"释放已读字段"的代码（WASM 体积）。
struct Fields {
    map: Map<String, Value>,
    error: Option<String>,
}

impl Fields {
    fn new(value: Value) -> Result<Self, String> {
        match value {
            Value::Object(map) => Ok(Fields { map, error: None }),
            _ => Err("应为对象".to_owned()),
        }
    }

    /// 记录错误（只保留第一条）。
    fn fail(&mut self, message: String) {
        if self.error.is_none() {
            self.error = Some(message);
        }
    }

    /// 读完所有字段后调用：有错误时返回第一条，否则返回 `value`。
    fn finish<T>(self, value: T) -> Result<T, String> {
        match self.error {
            Some(e) => Err(e),
            None => Ok(value),
        }
    }

    fn take(&mut self, key: &str) -> Option<Value> {
        self.map.remove(key).filter(|v| !v.is_null())
    }

    fn expect<T>(&mut self, key: &str, kind: &str, read: impl FnOnce(Value) -> Option<T>) -> Option<T> {
        let read = read(self.take(key)?);
        if read.is_none() {
            self.fail(format!("字段 {key} 应为{kind}"));
        }
        read
    }

    fn string(&mut self, key: &str) -> Option<String> {
        self.expect(key, "字符串", |v| match v {
            Value::String(s) => Some(s),
            _ => None,
        })
    }

    /// 缺失时记录错误并返回空字符串。
    fn required_string(&mut self, key: &str) -> String {
        let Some(s) = self.string(key) else {
            self.fail(format!("缺少字段 {key}"));
            return String::new();
        };
        s
    }

    fn u64(&mut self, key: &str) -> Option<u64> {
        self.expect(key, "非负整数", |v| v.as_u64())
    }

    fn u32(&mut self, key: &str) -> Option<u32> {
        self.expect(key, "非负整数（至多 4294967295）", |v| v.as_u64().and_then(|n| u32::try_from(n).ok()))
    }

    fn f64(&mut self, key: &str) -> Option<f64> {
        self.expect(key, "数字", |v| v.as_f64())
    }

    fn bool(&mut self, key: &str) -> Option<bool> {
        self.expect(key, "布尔值", |v| v.as_bool())
    }

    fn strings(&mut self, key: &str) -> Option<Vec<String>> {
        self.expect(key, "字符串数组", |v| match v {
            Value::Array(items) => items
                .into_iter()
                .map(|item| match item {
                    Value::String(s) => Some(s),
                    _ => None,
                })
                .collect(),
            _ => None,
        })
    }

    /// 任意 JSON 值（`null` 视为缺省）。
    fn value(&mut self, key: &str) -> Option<Value> {
        self.take(key)
    }

    /// 区分缺省（`None`）与显式 `null`（`Some(None)`）。
    fn nullable(&mut self, key: &str) -> Option<Option<Value>> {
        self.map.remove(key).map(|v| (!v.is_null()).then_some(v))
    }

    /// 可清除的字符串：缺省为 `None`，`null` 为 `Some(None)`。
    fn nullable_string(&mut self, key: &str) -> Option<Option<String>> {
        match self.nullable(key)? {
            None => Some(None),
            Some(Value::String(s)) => Some(Some(s)),
            Some(_) => {
                self.fail(format!("字段 {key} 应为字符串"));
                None
            }
        }
    }

    /// 嵌套对象，由 `T` 读取。
    fn object<T: FromJson>(&mut self, key: &str) -> Option<T> {
        match T::from_json(self.take(key)?) {
            Ok(v) => Some(v),
            Err(e) => {
                self.fail(format!("{key}.{e}"));
                None
            }
        }
    }

    /// 协议类型（枚举、`AppOverview`、`WakeDescriptor`）：沿用其 serde 定义。
    fn protocol<T: DeserializeOwned>(&mut self, key: &str) -> Option<T> {
        let value = self.take(key)?;
        self.protocol_value(key, value)
    }

    fn protocol_value<T: DeserializeOwned>(&mut self, key: &str, value: Value) -> Option<T> {
        match serde_json::from_value(value) {
            Ok(v) => Some(v),
            Err(e) => {
                self.fail(format!("字段 {key}：{e}"));
                None
            }
        }
    }

    /// 字符串枚举：`parse` 不认识时记录 `<无效说明>："<值>"`。
    fn keyword<T>(&mut self, key: &str, invalid: &str, parse: fn(&str) -> Option<T>) -> Option<T> {
        let s = self.string(key)?;
        let parsed = parse(&s);
        if parsed.is_none() {
            self.fail(format!("{invalid}：\"{s}\""));
        }
        parsed
    }
}
