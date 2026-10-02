//! 拨号得到字节流后、交给 App 连接服务前读取 App 的第一段数据（spec/naming.md 4.3"拒绝"，4.4 同样适用）：
//! SDK 总是先发 HTTP 升级请求（`GET /app`），App 拒绝时则写一行 `<CODE>：<说明>` 后断开，二者不会混淆。
//! 读到的升级请求原样回放（[`Replay`]）。用于没有方法错误可回的传输：Windows 命名管道、macOS launchd 套接字。

use std::pin::Pin;
use std::task::{Context, Poll};

use app_mcp_protocol::naming::{Address, codes, pipe as names};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf};
use tokio::time::Instant;

use super::{ChannelIo, ConnectorError};

/// HTTP 升级请求的开头（SDK 是 WebSocket 客户端，先发 `GET /app`）。
const UPGRADE_PREFIX: &[u8] = b"GET ";

/// 读第一段数据：升级请求 → 返回回放该开头的流；拒绝行 → 其中的错误码；超时 → `ACTIVATION_TIMEOUT`；
/// 未发送任何数据即关闭 → `ACTIVATION_DENIED`。
pub(crate) async fn expect_upgrade(
    mut stream: Box<dyn ChannelIo>,
    address: &Address,
    deadline: Instant,
) -> Result<Box<dyn ChannelIo>, ConnectorError> {
    let head = first_bytes(stream.as_mut(), address, deadline).await?;
    Ok(Box::new(Replay { head, pos: 0, inner: stream }))
}

/// 读到 App 发来的第一段数据：HTTP 升级请求的开头 → 返回已读字节（之后回放）；其他内容 → App 的拒绝行。
async fn first_bytes(stream: &mut dyn ChannelIo, address: &Address, deadline: Instant) -> Result<Vec<u8>, ConnectorError> {
    let mut head = Vec::new();
    let mut buf = [0u8; 512];
    loop {
        let n = match tokio::time::timeout_at(deadline, stream.read(&mut buf)).await {
            Err(_) => {
                return Err(ConnectorError::new(codes::ACTIVATION_TIMEOUT, format!("{address} 打开通道后没有及时发送握手")));
            }
            Ok(Ok(n)) => n,
            // 管道断开（ERROR_BROKEN_PIPE）、连接被重置等与 EOF 同样处理。
            Ok(Err(_)) => 0,
        };
        head.extend_from_slice(&buf[..n]);
        if head.len() >= UPGRADE_PREFIX.len() && head.starts_with(UPGRADE_PREFIX) {
            return Ok(head);
        }
        let complete = n == 0 || head.contains(&b'\n') || head.len() >= names::MAX_REFUSAL_LINE;
        if !complete {
            continue;
        }
        if head.is_empty() {
            return Err(ConnectorError::new(codes::ACTIVATION_DENIED, format!("{address} 在发送握手前关闭了通道")));
        }
        if UPGRADE_PREFIX.starts_with(&head) {
            return Err(ConnectorError::new(codes::ACTIVATION_DENIED, format!("{address} 的握手在开头处中断")));
        }
        let line = String::from_utf8_lossy(&head);
        let (code, detail) = names::parse_refusal(line.lines().next().unwrap_or_default());
        return Err(ConnectorError::new(code, format!("{address} 拒绝了通道：{detail}")));
    }
}

/// 先回放已读的开头，再读底层流。
struct Replay {
    head: Vec<u8>,
    pos: usize,
    inner: Box<dyn ChannelIo>,
}

impl AsyncRead for Replay {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        let this = &mut *self;
        if let Some(rest) = this.head.get(this.pos..).filter(|r| !r.is_empty()) {
            let n = rest.len().min(buf.remaining());
            buf.put_slice(&rest[..n]);
            this.pos += n;
            if this.pos == this.head.len() {
                this.head = Vec::new();
                this.pos = 0;
            }
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut this.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for Replay {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}
