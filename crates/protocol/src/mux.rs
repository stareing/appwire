//! 连接多路复用（spec/protocol.md 第 9 节）。
//!
//! 一条传输连接默认只承载一个实例。SDK 以 `app/mux` 作为连接上的**第一条消息**协商成功后，
//! 该连接改为承载多个**通道**：每个通道等同于一条独立的 v1 连接（各自 `app/hello`、心跳、休眠、断开），
//! 帧格式见 [`MuxFrame`]。用途：同一来源的多个浏览器标签页经 SharedWorker 共用一条 WebSocket，
//! 每个标签页仍是独立实例（工具随标签页存亡）。
//!
//! 旧 Host 不认识 `app/mux`，按"握手前的请求"返回 `UNAUTHORIZED` 错误；SDK 据此判定不支持，
//! 改为每个实例各开一条连接（能力协商，不是失败回退）。

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

/// 当前多路复用版本。
pub const MUX_VERSION: u32 = 1;

/// Host 在一条多路复用连接上同时接受的通道数上限（[`MuxResult::max_channels`] 的缺省值）。
pub const MUX_MAX_CHANNELS: u32 = 64;

/// `app/mux` 请求参数。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MuxParams {
    /// SDK 支持的最高版本。
    pub version: u32,
}

/// `app/mux` 结果：连接已切换为多路复用模式。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MuxResult {
    /// 双方采用的版本（≤ 请求中的版本）。
    pub version: u32,
    /// 同时打开的通道数上限；超出时 Host 以 `close` 帧拒绝新通道。
    pub max_channels: u32,
}

/// 多路复用模式下的一帧（一个 WebSocket 文本帧），JSON 形如 `{"type":"open"|"msg"|"close","ch":N,…}`。
///
/// - `open`：SDK 打开通道 `ch`（通道号由 SDK 选择，≥ 1，同一连接内不重复使用）。之后该通道上的第一条消息
///   必须是 `app/hello`，与新连接相同。
/// - `msg`：通道 `ch` 上的一条 v1 消息（JSON-RPC 对象，原样嵌入 `msg` 字段）。
/// - `close`：任一方关闭通道，等同于该通道的连接断开；`reason` 仅用于诊断。
///   关闭后收到的、发往该通道的帧直接丢弃。
#[derive(Clone, Debug)]
pub enum MuxFrame {
    Open { ch: u32 },
    Msg { ch: u32, msg: Box<RawValue> },
    Close { ch: u32, reason: Option<String> },
}

/// 线上格式（内部标签枚举无法承载 `RawValue`，故用扁平结构手工分派）。
#[derive(Serialize, Deserialize)]
struct Wire<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
    ch: u32,
    #[serde(borrow, default, skip_serializing_if = "Option::is_none")]
    msg: Option<&'a RawValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

impl MuxFrame {
    /// 解析一帧。
    pub fn parse(text: &str) -> Result<Self, serde_json::Error> {
        use serde::de::Error as _;
        let w: Wire<'_> = serde_json::from_str(text)?;
        match (w.kind, w.msg) {
            ("open", _) => Ok(MuxFrame::Open { ch: w.ch }),
            ("msg", Some(msg)) => Ok(MuxFrame::Msg { ch: w.ch, msg: msg.to_owned() }),
            ("msg", None) => Err(serde_json::Error::custom("msg 帧缺少 msg 字段")),
            ("close", _) => Ok(MuxFrame::Close { ch: w.ch, reason: w.reason }),
            (other, _) => Err(serde_json::Error::custom(format!("未知的帧类型 {other}"))),
        }
    }

    /// 序列化为帧文本。
    pub fn to_text(&self) -> String {
        let w = match self {
            MuxFrame::Open { ch } => Wire { kind: "open", ch: *ch, msg: None, reason: None },
            MuxFrame::Msg { ch, msg } => Wire { kind: "msg", ch: *ch, msg: Some(msg), reason: None },
            MuxFrame::Close { ch, reason } => Wire { kind: "close", ch: *ch, msg: None, reason: reason.clone() },
        };
        // 字段只有字符串、整数与已校验的 RawValue，序列化不会失败。
        serde_json::to_string(&w).unwrap_or_default()
    }

    /// 通道号。
    pub fn channel(&self) -> u32 {
        match self {
            MuxFrame::Open { ch } | MuxFrame::Msg { ch, .. } | MuxFrame::Close { ch, .. } => *ch,
        }
    }

    /// 把一条 v1 消息（JSON 文本）包装为 `msg` 帧；`message` 不是合法 JSON 时报错。
    pub fn wrap(ch: u32, message: &str) -> Result<String, serde_json::Error> {
        let msg = RawValue::from_string(message.to_owned())?;
        Ok(MuxFrame::Msg { ch, msg }.to_text())
    }

    /// `close` 帧的文本。
    pub fn close_text(ch: u32, reason: Option<&str>) -> String {
        MuxFrame::Close { ch, reason: reason.map(str::to_owned) }.to_text()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_roundtrip() {
        let text = MuxFrame::wrap(3, r#"{"jsonrpc":"2.0","method":"app/ready","params":{}}"#).unwrap();
        assert_eq!(text, r#"{"type":"msg","ch":3,"msg":{"jsonrpc":"2.0","method":"app/ready","params":{}}}"#);
        let MuxFrame::Msg { ch, msg } = MuxFrame::parse(&text).unwrap() else { panic!("expected msg") };
        assert_eq!(ch, 3);
        assert_eq!(msg.get(), r#"{"jsonrpc":"2.0","method":"app/ready","params":{}}"#);

        let open = MuxFrame::parse(r#"{"type":"open","ch":1}"#).unwrap();
        assert!(matches!(open, MuxFrame::Open { ch: 1 }));
        assert_eq!(open.channel(), 1);

        assert_eq!(MuxFrame::close_text(2, None), r#"{"type":"close","ch":2}"#);
        assert_eq!(MuxFrame::close_text(2, Some("超出上限")), r#"{"type":"close","ch":2,"reason":"超出上限"}"#);
        let MuxFrame::Close { ch, reason } = MuxFrame::parse(r#"{"type":"close","ch":9}"#).unwrap() else {
            panic!("expected close")
        };
        assert_eq!((ch, reason), (9, None));
    }

    #[test]
    fn rejects_bad_frames() {
        assert!(MuxFrame::parse(r#"{"type":"nope","ch":1}"#).is_err());
        assert!(MuxFrame::parse(r#"{"type":"open"}"#).is_err());
        assert!(MuxFrame::parse(r#"{"type":"open","ch":-1}"#).is_err());
        assert!(MuxFrame::wrap(1, "not json").is_err());
    }

    #[test]
    fn negotiation_types() {
        let p: MuxParams = serde_json::from_str(r#"{"version":1}"#).unwrap();
        assert_eq!(p.version, MUX_VERSION);
        let r = serde_json::to_string(&MuxResult { version: 1, max_channels: MUX_MAX_CHANNELS }).unwrap();
        assert_eq!(r, r#"{"version":1,"maxChannels":64}"#);
    }
}
