//! 按需启动（spec/protocol.md 1.9）：服务管理器预先绑定的监听器，与"空闲后退出"的判定。
//!
//! - [`PreboundListeners`]：systemd 套接字激活（`LISTEN_FDS`）/ launchd `Sockets` 交来的监听套接字，
//!   [`crate::Hub::start_with`] 用它们代替自己绑定。监听端口与套接字文件归服务管理器所有：Hub 不删除套接字文件，
//!   退出后新连接留在内核队列中，由服务管理器再次启动 Host 接受。
//! - [`Activity`]：接受闸门与连接计数。每条已接受的连接（含其上的请求、SSE 流、升级后的 App WebSocket）从接受到结束
//!   计一次；[`Activity::wait_idle`] 等到计数为 0 且其他占用（进行中的调用 / 唤醒、在线 App、MCP 会话）都没有、并持续
//!   `idle` 后，关闭闸门（接受循环停在 accept 之外）再复核一次，确认没有漏进来的连接才返回。
//!
//! @why 空闲期间只有一个一次性定时器（`idle` 倒计时），连接进出由 `watch` 通知驱动，不轮询；倒计时到点时若仍有非连接类
//! 占用（如按名拨入的通道），重新倒计时（每 `idle` 至多一次唤醒）。只有调用 `wait_idle` 的进程（Host 按需模式）才有这个定时器。

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;

/// 服务管理器交来的、已在监听的套接字（[`crate::Hub::start_with`]）。字段为 `None` 时按 [`crate::HubConfig`] 自己绑定。
#[derive(Debug, Default)]
pub struct PreboundListeners {
    /// 代替 [`crate::HubConfig::listen`]（`/app`、`/mcp`、`/healthz`）；同样受回环检查约束。
    pub tcp: Option<std::net::TcpListener>,
    /// 代替 [`crate::HubConfig::ipc_endpoint`]；端点按套接字的路径填写（`unix:<路径>`），Hub 不删除该文件。
    #[cfg(unix)]
    pub ipc: Option<std::os::unix::net::UnixListener>,
}

impl PreboundListeners {
    /// 是否有任何预先绑定的监听器。
    pub fn is_empty(&self) -> bool {
        #[cfg(unix)]
        let ipc = self.ipc.is_none();
        #[cfg(not(unix))]
        let ipc = true;
        self.tcp.is_none() && ipc
    }

    /// 取出 TCP 监听器并转为 tokio 的（非阻塞）。
    pub(crate) fn take_tcp(&mut self) -> std::io::Result<Option<tokio::net::TcpListener>> {
        let Some(l) = self.tcp.take() else { return Ok(None) };
        l.set_nonblocking(true)?;
        tokio::net::TcpListener::from_std(l).map(Some)
    }

    /// 取出本地 IPC 监听器：`(端点字符串, 监听器)`。
    ///
    /// @error 套接字没有文件路径（匿名 / 抽象命名空间）或所在目录不安全时返回错误（与自己绑定时同样的目录要求）。
    pub(crate) fn take_ipc(&mut self) -> std::io::Result<Option<(String, crate::ipc::IpcListener)>> {
        #[cfg(unix)]
        {
            let Some(l) = self.ipc.take() else { return Ok(None) };
            crate::ipc::IpcListener::adopt(l).map(Some)
        }
        #[cfg(not(unix))]
        {
            Ok(None)
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Gate {
    /// 已接受、尚未结束的连接数（含升级后的 App WebSocket）。
    open: usize,
    /// 正在 accept 的接受循环数（持有 [`AcceptPermit`]）。
    accepting: usize,
    /// 闸门关闭：接受循环不再 accept。
    draining: bool,
    /// 累计计入的连接数（只增）：倒计时期间开始并已结束的短连接也要让倒计时重来（`watch` 只保留最新值）。
    seq: u64,
}

/// 接受闸门与连接计数（[`crate::hub::HubShared`] 持有一份，所有监听器共用）。
#[derive(Clone, Debug, Default)]
pub(crate) struct Activity {
    gate: Arc<watch::Sender<Gate>>,
}

/// 一次 accept 的许可：闸门关闭时 [`Activity::permit`] 等待；许可存续期间 [`Activity::wait_idle`] 不会认定"已停止接受"。
pub(crate) struct AcceptPermit {
    gate: Arc<watch::Sender<Gate>>,
    admitted: bool,
}

/// 一条连接的计数；丢弃时减一。
#[derive(Debug)]
pub(crate) struct ConnectionGuard {
    gate: Arc<watch::Sender<Gate>>,
}

impl Activity {
    /// 取得一次 accept 的许可；闸门关闭期间等待重新打开。
    pub(crate) async fn permit(&self) -> AcceptPermit {
        let mut rx = self.gate.subscribe();
        loop {
            let mut granted = false;
            self.gate.send_if_modified(|g| {
                granted = !g.draining;
                if granted {
                    g.accepting += 1;
                }
                granted
            });
            if granted {
                return AcceptPermit { gate: self.gate.clone(), admitted: false };
            }
            if rx.wait_for(|g| !g.draining).await.is_err() {
                // 发送端与 self 同生命周期，不会先于 self 丢弃；保守起见仍给出许可。
                return AcceptPermit { gate: self.gate.clone(), admitted: false };
            }
        }
    }

    /// 闸门关闭时完成（接受循环据此放弃正在等待的 accept；tokio 的 accept 可安全取消，连接留在内核队列）。
    pub(crate) async fn draining(&self) {
        let mut rx = self.gate.subscribe();
        let _ = rx.wait_for(|g| g.draining).await;
    }

    /// 为已有连接上派生的长连接（升级后的 App WebSocket）再计一次。
    pub(crate) fn hold(&self) -> ConnectionGuard {
        self.gate.send_modify(|g| {
            g.open += 1;
            g.seq += 1;
        });
        ConnectionGuard { gate: self.gate.clone() }
    }

    /// 等到空闲满 `idle` 并关闭闸门后返回（之后不再接受新连接，调用方应停止 Hub）。
    ///
    /// @input `blocker` 返回连接计数之外的占用原因（进行中的调用 / 唤醒、在线 App、MCP 会话）；`None` = 无占用。
    /// @invariant 返回时闸门已关闭、没有接受循环在 accept、连接计数为 0、`blocker` 为 `None`。
    pub(crate) async fn wait_idle(&self, idle: Duration, blocker: impl Fn() -> Option<String>) {
        let mut rx = self.gate.subscribe();
        loop {
            let seq = match rx.wait_for(|g| g.open == 0).await {
                Ok(g) => g.seq,
                Err(_) => return,
            };
            let quiet = tokio::select! {
                () = tokio::time::sleep(idle) => true,
                _ = rx.wait_for(|g| g.open > 0 || g.seq != seq) => false,
            };
            if !quiet {
                continue;
            }
            if let Some(why) = blocker() {
                tracing::debug!("空闲倒计时到点，但仍有占用（{why}），重新计时");
                continue;
            }
            self.gate.send_modify(|g| g.draining = true);
            if rx.wait_for(|g| g.accepting == 0).await.is_err() {
                return;
            }
            let open = rx.borrow().open;
            match blocker() {
                None if open == 0 => return,
                why => {
                    tracing::debug!(open, ?why, "关闭接受闸门时有新的活动，继续服务");
                    self.gate.send_modify(|g| g.draining = false);
                }
            }
        }
    }

    #[cfg(test)]
    fn snapshot(&self) -> Gate {
        *self.gate.borrow()
    }
}

impl AcceptPermit {
    /// accept 成功：把许可换成连接计数（同一次原子更新，闸门关闭的复核不会漏掉这条连接）。
    pub(crate) fn admit(mut self) -> ConnectionGuard {
        self.admitted = true;
        self.gate.send_modify(|g| {
            g.accepting -= 1;
            g.open += 1;
            g.seq += 1;
        });
        ConnectionGuard { gate: self.gate.clone() }
    }
}

impl Drop for AcceptPermit {
    fn drop(&mut self) {
        if !self.admitted {
            self.gate.send_modify(|g| g.accepting -= 1);
        }
    }
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.gate.send_modify(|g| g.open -= 1);
    }
}

impl crate::hub::HubShared {
    /// 连接计数之外让 Host 不能空闲退出的占用（[`Activity::wait_idle`] 的 `blocker`）；`None` = 无。
    ///
    /// @why 按名拨入的通道（spec/naming.md）不经监听器，进行中的调用 / 唤醒可能跨越连接，旧式 MCP 会话的状态
    /// （`apps.select`、租约）在进程内：这些都在时退出会丢工作或状态。
    pub(crate) fn idle_blocker(&self) -> Option<String> {
        let calls = crate::hub::lock(&self.calls).len();
        if calls > 0 {
            return Some(format!("{calls} 个进行中的调用"));
        }
        let wakes = crate::hub::lock(&self.wakes).len();
        if wakes > 0 {
            return Some(format!("{wakes} 个进行中的唤醒"));
        }
        let online = self.registry().all_connections().len();
        if online > 0 {
            return Some(format!("{online} 个在线 App 实例"));
        }
        let sessions = self.mcp_session_count();
        (sessions > 0).then(|| format!("{sessions} 个 MCP 会话"))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Instant;

    use super::*;

    const IDLE: Duration = Duration::from_millis(80);

    fn never() -> Option<String> {
        None
    }

    #[tokio::test]
    async fn idle_without_connections_returns_after_idle_and_closes_gate() {
        let a = Activity::default();
        let started = Instant::now();
        a.wait_idle(IDLE, never).await;
        assert!(started.elapsed() >= IDLE);
        assert_eq!(a.snapshot(), Gate { open: 0, accepting: 0, draining: true, seq: 0 });
    }

    #[tokio::test]
    async fn open_connection_blocks_until_closed_then_idle_again() {
        let a = Activity::default();
        let guard = a.hold();
        let waiter = tokio::spawn({
            let a = a.clone();
            async move { a.wait_idle(IDLE, never).await }
        });
        tokio::time::sleep(IDLE * 3).await;
        assert!(!waiter.is_finished(), "连接未结束时不应空闲退出");
        let closed_at = Instant::now();
        drop(guard);
        waiter.await.unwrap();
        assert!(closed_at.elapsed() >= IDLE, "连接结束后应再满一个空闲时长");
    }

    #[tokio::test]
    async fn activity_during_countdown_restarts_it() {
        let a = Activity::default();
        let waiter = tokio::spawn({
            let a = a.clone();
            async move { a.wait_idle(IDLE, never).await }
        });
        tokio::time::sleep(IDLE / 2).await;
        drop(a.hold());
        let last = Instant::now();
        waiter.await.unwrap();
        assert!(last.elapsed() >= IDLE, "倒计时应从最后一次活动重新开始");
    }

    #[tokio::test]
    async fn blocker_rearms_countdown() {
        let a = Activity::default();
        let busy = Arc::new(AtomicBool::new(true));
        let waiter = tokio::spawn({
            let (a, busy) = (a.clone(), busy.clone());
            async move { a.wait_idle(IDLE, move || busy.load(Ordering::SeqCst).then(|| "App 在线".to_owned())).await }
        });
        tokio::time::sleep(IDLE * 3).await;
        assert!(!waiter.is_finished(), "有占用时不应退出");
        assert!(!a.snapshot().draining, "有占用时不应关闭闸门");
        busy.store(false, Ordering::SeqCst);
        waiter.await.unwrap();
    }

    #[tokio::test]
    async fn gate_waits_for_in_progress_accept_and_counts_admitted_connection() {
        let a = Activity::default();
        // 一个接受循环正持有许可（accept 刚返回、尚未登记连接）
        let permit = a.permit().await;
        let waiter = tokio::spawn({
            let a = a.clone();
            async move { a.wait_idle(IDLE, never).await }
        });
        tokio::time::sleep(IDLE * 2).await;
        assert!(a.snapshot().draining, "倒计时到点后应关闭闸门");
        assert!(!waiter.is_finished(), "许可未交回前不能认定已停止接受");
        // 这次 accept 得到了连接：计数加一，闸门重新打开，继续服务
        let guard = permit.admit();
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!waiter.is_finished(), "漏进来的连接必须被服务完");
        assert!(!a.snapshot().draining, "复核发现新连接后闸门应重新打开");
        drop(guard);
        waiter.await.unwrap();
    }

    #[tokio::test]
    async fn permit_waits_while_draining() {
        let a = Activity::default();
        a.gate.send_modify(|g| g.draining = true);
        let pending = tokio::spawn({
            let a = a.clone();
            async move { drop(a.permit().await) }
        });
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(!pending.is_finished(), "闸门关闭时不应发放许可");
        a.gate.send_modify(|g| g.draining = false);
        pending.await.unwrap();
        assert_eq!(a.snapshot().accepting, 0);
    }

    #[tokio::test]
    async fn draining_future_completes_when_gate_closes() {
        let a = Activity::default();
        let d = tokio::spawn({
            let a = a.clone();
            async move { a.draining().await }
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!d.is_finished());
        a.gate.send_modify(|g| g.draining = true);
        d.await.unwrap();
    }

    /// Hub 在交来的监听器上服务（配置的地址被忽略）；连接未结束不空闲；空闲后闸门关闭，新连接不再被接受（留在内核队列）。
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn hub_serves_on_prebound_listeners_and_stops_accepting_when_idle() {
        use std::io::{Read, Write};
        use std::os::unix::fs::PermissionsExt;

        let tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = tcp.local_addr().unwrap();
        let dir = std::env::temp_dir().join(format!("amcp-prebound-{:016x}", rand::random::<u64>()));
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let sock = dir.join("hub.sock");
        let ipc = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        let config = crate::HubConfig {
            // @why 端口 1 无权绑定：若 Hub 没用交来的监听器而去绑定配置地址，启动即失败。
            listen: Some("127.0.0.1:1".into()),
            listen_alternates: Vec::new(),
            ipc_endpoint: None,
            ..Default::default()
        };
        let hub = Arc::new(
            crate::Hub::start_with(config, PreboundListeners { tcp: Some(tcp), ipc: Some(ipc) }).await.unwrap(),
        );
        assert_eq!(hub.listen_addr(), Some(addr));
        assert_eq!(hub.ipc_endpoint(), Some(format!("unix:{}", sock.display()).as_str()));

        let healthz = |s: &mut std::net::TcpStream| {
            s.write_all(b"GET /healthz HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").unwrap();
            let mut buf = [0u8; 12];
            s.read_exact(&mut buf).unwrap();
            String::from_utf8_lossy(&buf).into_owned()
        };
        let mut kept = std::net::TcpStream::connect(addr).unwrap();
        assert_eq!(healthz(&mut kept), "HTTP/1.1 200");
        let waiter = tokio::spawn({
            let hub = hub.clone();
            async move { hub.wait_idle(IDLE).await }
        });
        tokio::time::sleep(IDLE * 3).await;
        assert!(!waiter.is_finished(), "保持中的连接应阻止空闲退出");
        drop(kept);
        tokio::time::timeout(Duration::from_secs(5), waiter).await.unwrap().unwrap();

        // 闸门已关闭：内核仍完成握手（监听套接字归服务管理器），但 Hub 不再读取请求
        let mut late = std::net::TcpStream::connect(addr).unwrap();
        late.set_read_timeout(Some(Duration::from_millis(300))).unwrap();
        late.write_all(b"GET /healthz HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").unwrap();
        let mut buf = [0u8; 1];
        let e = late.read(&mut buf).unwrap_err();
        assert!(matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut), "{e}");
        Arc::into_inner(hub).unwrap().shutdown().await;
        assert!(sock.exists(), "交来的套接字文件归服务管理器，Hub 不删除");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    // @why Windows 上 PreboundListeners 只有 tcp 一个字段，`..Default::default()` 在那里多余；其他平台需要它。
    #[cfg_attr(windows, allow(clippy::needless_update))]
    fn prebound_is_empty() {
        assert!(PreboundListeners::default().is_empty());
        let tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        assert!(!PreboundListeners { tcp: Some(tcp), ..Default::default() }.is_empty());
    }
}
