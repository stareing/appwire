//! 按需启动（spec/protocol.md 1.9）：取得服务管理器交来的监听套接字。
//!
//! - Linux：systemd 套接字激活。按 `sd_listen_fds(3)` 协议直接读环境变量（不依赖 libsystemd）：`LISTEN_PID` 必须等于本进程，
//!   `LISTEN_FDS` 个 fd 从 3 开始，`LISTEN_FDNAMES` 为冒号分隔的名字（只用于日志）。取到后清除这三个变量、给 fd 设
//!   `FD_CLOEXEC`（上游 MCP 子进程不继承监听套接字）。
//! - macOS：launchd `Sockets`，键 [`LAUNCHD_HTTP_KEY`] / [`LAUNCHD_IPC_KEY`]，经 `launch_activate_socket`
//!   （[`app_mcp_protocol::naming::launchd::activate_socket`]，与 App 侧共用的唯一 FFI）。不是由 launchd 启动、或 plist
//!   没有 `Sockets`（登录自启方式）时视为未激活。
//! - 其他平台（Windows）：没有对等机制，总是未激活。
//!
//! 交来的 fd 按地址族分类（`getsockname`）：IPv4 / IPv6 → HTTP 监听（`/app`、`/mcp`、`/healthz`），Unix → 本地 IPC。
//!
//! @why 不按名字分类：systemd 的 `FileDescriptorName=` 作用于整个 `.socket` 单元的所有 fd（systemd.socket(5)），
//! 同一单元里的 TCP 与 Unix 套接字名字相同；按地址族分类与名字无关，单元写法也更简单。

use app_mcp_hub::PreboundListeners;

/// systemd 交来的第一个 fd（`SD_LISTEN_FDS_START`）。
pub const SD_LISTEN_FDS_START: i32 = 3;
/// 最多接受的 fd 数（Host 只需要 HTTP + IPC 两个；超出视为配置错误，G-07）。
pub const MAX_LISTEN_FDS: usize = 8;
/// systemd 套接字单元的 `FileDescriptorName`（[`crate::service::systemd_socket_unit`] 写入，只用于日志）。
pub const SYSTEMD_FD_NAME: &str = "app-mcp-host";
/// launchd plist `Sockets` 中 HTTP 监听的键（[`crate::service::launchd_plist`] 写入）。
pub const LAUNCHD_HTTP_KEY: &str = "Http";
/// launchd plist `Sockets` 中本地 IPC 的键。
pub const LAUNCHD_IPC_KEY: &str = "Ipc";
/// 按需启动时的空闲退出时间缺省值（`lifecycle.idleExitMs` / `--idle-exit-ms`）。
///
/// @why 10 分钟：与无会话 MCP 任务的空闲回收（`HubConfig::task_idle_ttl`）一致，覆盖一轮对话中的思考停顿，避免同一轮对话里
/// 反复冷启动（冷启动要重新读清单、拉起上游 MCP 服务器）；空闲 Host 的常驻成本见 crates/host/README.md「按需启动」。
pub const DEFAULT_IDLE_EXIT_MS: u64 = 10 * 60 * 1000;

/// 交来监听套接字的服务管理器。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Systemd,
    Launchd,
}

impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Systemd => "systemd 套接字激活",
            Self::Launchd => "launchd Sockets",
        })
    }
}

/// 服务管理器交来的监听器。
#[derive(Debug)]
pub struct Inherited {
    pub source: Source,
    pub listeners: PreboundListeners,
    /// 每个监听器的说明（日志用），如 `127.0.0.1:7717（app-mcp-host）`。
    pub described: Vec<String>,
}

/// 取得交来的监听器失败。
#[derive(Debug, PartialEq, Eq)]
pub enum ActivationError {
    /// 环境变量不合法（`LISTEN_PID` / `LISTEN_FDS` 不是数字、fd 数超过 [`MAX_LISTEN_FDS`]）。
    Env(String),
    /// 交来的 fd 不可用（已关闭、不是监听中的流套接字、地址族不支持、同类监听器多于一个）。
    Socket(String),
    /// launchd 返回的错误（除"不是由 launchd 启动" / "没有该键"之外）。
    Launchd(String),
}

impl std::fmt::Display for ActivationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Env(m) => write!(f, "套接字激活的环境变量不合法：{m}"),
            Self::Socket(m) => write!(f, "服务管理器交来的套接字不可用：{m}"),
            Self::Launchd(m) => write!(f, "launchd 套接字激活失败：{m}"),
        }
    }
}

impl std::error::Error for ActivationError {}

/// `sd_listen_fds` 环境变量的解析结果：fd 为 `SD_LISTEN_FDS_START .. SD_LISTEN_FDS_START + names.len()`。
#[derive(Debug, PartialEq, Eq)]
pub struct ListenFds {
    /// 每个 fd 的名字；`LISTEN_FDNAMES` 缺失或个数不符时为 `"unknown"`（与 `sd_listen_fds_with_names` 相同）。
    pub names: Vec<String>,
}

/// 解析 `LISTEN_PID` / `LISTEN_FDS` / `LISTEN_FDNAMES`（纯函数）。
///
/// @output `Ok(None)`：没有交来 fd（变量缺失、`LISTEN_FDS=0`，或 `LISTEN_PID` 不是本进程——变量是给别的进程的）。
/// @error 变量存在但不是数字、fd 数超过 [`MAX_LISTEN_FDS`]。
pub fn parse_listen_env(
    my_pid: u32,
    listen_pid: Option<&str>,
    listen_fds: Option<&str>,
    fd_names: Option<&str>,
) -> Result<Option<ListenFds>, ActivationError> {
    let (Some(pid), Some(fds)) = (listen_pid, listen_fds) else {
        return Ok(None);
    };
    let pid: u32 = pid
        .trim()
        .parse()
        .map_err(|_| ActivationError::Env(format!("LISTEN_PID 不是进程号：{pid:?}")))?;
    if pid != my_pid {
        return Ok(None);
    }
    let count: usize = fds
        .trim()
        .parse()
        .map_err(|_| ActivationError::Env(format!("LISTEN_FDS 不是数字：{fds:?}")))?;
    if count == 0 {
        return Ok(None);
    }
    if count > MAX_LISTEN_FDS {
        return Err(ActivationError::Env(format!("LISTEN_FDS={count} 超过上限 {MAX_LISTEN_FDS}")));
    }
    let names: Vec<String> = match fd_names.map(|n| n.split(':').map(str::to_owned).collect::<Vec<_>>()) {
        Some(n) if n.len() == count => n,
        _ => vec!["unknown".to_owned(); count],
    };
    Ok(Some(ListenFds { names }))
}

/// 交来的一个监听套接字。
#[cfg(unix)]
enum Classified {
    Tcp(std::net::TcpListener),
    Unix(std::os::unix::net::UnixListener),
}

/// 按地址族分类（`getsockname`）。
///
/// @error 不是监听中的流套接字、地址族不是 IPv4 / IPv6 / Unix。
#[cfg(unix)]
fn classify(fd: std::os::fd::OwnedFd) -> Result<(Classified, String), ActivationError> {
    if !sys::is_listening(&fd) {
        return Err(ActivationError::Socket("不是监听中的流套接字（检查单元的 ListenStream / plist 的 SockType）".into()));
    }
    let tcp = std::net::TcpListener::from(fd);
    if let Ok(addr) = tcp.local_addr() {
        return Ok((Classified::Tcp(tcp), addr.to_string()));
    }
    let unix = std::os::unix::net::UnixListener::from(std::os::fd::OwnedFd::from(tcp));
    match unix.local_addr() {
        Ok(addr) => {
            let text = addr.as_pathname().map_or_else(|| "（无路径）".to_owned(), |p| p.display().to_string());
            Ok((Classified::Unix(unix), text))
        }
        Err(e) => Err(ActivationError::Socket(format!("地址族不支持（只接受 IPv4 / IPv6 / Unix）：{e}"))),
    }
}

/// 把分类后的套接字放进 [`PreboundListeners`]；同类多于一个时报错（Hub 只服务一个 HTTP 与一个 IPC 监听器）。
#[cfg(unix)]
fn assemble(
    source: Source,
    fds: Vec<(std::os::fd::OwnedFd, String)>,
) -> Result<Option<Inherited>, ActivationError> {
    if fds.is_empty() {
        return Ok(None);
    }
    let mut listeners = PreboundListeners::default();
    let mut described = Vec::new();
    for (fd, name) in fds {
        let (classified, addr) = classify(fd)?;
        let slot_taken = match classified {
            Classified::Tcp(l) => listeners.tcp.replace(l).is_some(),
            Classified::Unix(l) => listeners.ipc.replace(l).is_some(),
        };
        if slot_taken {
            return Err(ActivationError::Socket(format!(
                "同类监听套接字多于一个（{addr}）：Host 只接受一个 TCP 与一个 Unix 监听"
            )));
        }
        described.push(format!("{addr}（{name}）"));
    }
    Ok(Some(Inherited { source, listeners, described }))
}

/// 取得服务管理器交来的监听器；没有交来时为 `Ok(None)`（普通 `serve`）。
///
/// @side-effect Linux：取到后清除 `LISTEN_PID` / `LISTEN_FDS` / `LISTEN_FDNAMES`，给 fd 设 `FD_CLOEXEC`。
/// 必须在创建其他线程之前调用（清除环境变量在多线程下不安全）。
pub fn take_inherited() -> Result<Option<Inherited>, ActivationError> {
    #[cfg(target_os = "linux")]
    {
        take_systemd()
    }
    #[cfg(target_os = "macos")]
    {
        take_launchd()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        Ok(None)
    }
}

#[cfg(target_os = "linux")]
fn take_systemd() -> Result<Option<Inherited>, ActivationError> {
    const VARS: [&str; 3] = ["LISTEN_PID", "LISTEN_FDS", "LISTEN_FDNAMES"];
    let [pid, fds, names] = VARS.map(|k| std::env::var(k).ok());
    let parsed = parse_listen_env(std::process::id(), pid.as_deref(), fds.as_deref(), names.as_deref());
    if matches!(parsed, Ok(Some(_))) {
        for k in VARS {
            // SAFETY: 由 [`take_inherited`] 的约定，此时进程内只有主线程（运行时尚未创建），没有并发读写环境变量。
            unsafe { std::env::remove_var(k) };
        }
    }
    let Some(listen) = parsed? else { return Ok(None) };
    let fds = listen
        .names
        .into_iter()
        .zip(SD_LISTEN_FDS_START..)
        .map(|(name, raw)| sys::adopt_listen_fd(raw).map(|fd| (fd, name)))
        .collect::<Result<Vec<_>, _>>()?;
    assemble(Source::Systemd, fds)
}

#[cfg(target_os = "macos")]
fn take_launchd() -> Result<Option<Inherited>, ActivationError> {
    use app_mcp_protocol::naming::launchd::{ACTIVATE_ENOENT, ACTIVATE_ESRCH, ActivateSocketError, activate_socket};
    let mut fds = Vec::new();
    for key in [LAUNCHD_HTTP_KEY, LAUNCHD_IPC_KEY] {
        match activate_socket(key) {
            Ok(got) => fds.extend(got.into_iter().map(|fd| (fd, key.to_owned()))),
            // 不是由 launchd 启动（手动 serve）/ plist 没有该键（登录自启方式、或未开 IPC）：不算错误。
            Err(ActivateSocketError::Os(ACTIVATE_ESRCH | ACTIVATE_ENOENT)) => {}
            Err(e) => return Err(ActivationError::Launchd(format!("键 {key}：{e:?}"))),
        }
    }
    assemble(Source::Launchd, fds)
}

/// 交来的 fd 的系统调用（G-06：本模块的 unsafe 只在这里）。
#[cfg(unix)]
mod sys {
    use std::os::fd::{AsRawFd, OwnedFd};

    /// `SO_ACCEPTCONN`：套接字是否处于监听状态。
    pub(super) fn is_listening(fd: &OwnedFd) -> bool {
        let mut value: libc::c_int = 0;
        let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
        // SAFETY: fd 有效（由 OwnedFd 保证）；value / len 指向本函数的局部变量，长度与类型一致。
        let rc = unsafe {
            libc::getsockopt(
                fd.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_ACCEPTCONN,
                (&mut value as *mut libc::c_int).cast(),
                &mut len,
            )
        };
        rc == 0 && value != 0
    }

    /// 接管 systemd 交来的 fd：确认它是打开的，设 `FD_CLOEXEC`，取得所有权。
    ///
    /// @error fd 未打开（单元配置与 `LISTEN_FDS` 不符）。
    #[cfg(target_os = "linux")]
    pub(super) fn adopt_listen_fd(raw: libc::c_int) -> Result<OwnedFd, super::ActivationError> {
        use std::os::fd::FromRawFd;
        // SAFETY: F_GETFD 只查询，不改变任何状态；fd 无效时返回 -1。
        let flags = unsafe { libc::fcntl(raw, libc::F_GETFD) };
        if flags < 0 {
            return Err(super::ActivationError::Socket(format!(
                "fd {raw} 未打开：{}",
                std::io::Error::last_os_error()
            )));
        }
        // SAFETY: raw 已确认是打开的 fd；设置 FD_CLOEXEC 只影响 exec 时是否继承。
        if unsafe { libc::fcntl(raw, libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0 {
            return Err(super::ActivationError::Socket(format!(
                "无法给 fd {raw} 设置 FD_CLOEXEC：{}",
                std::io::Error::last_os_error()
            )));
        }
        // SAFETY: 按 sd_listen_fds(3) 约定，LISTEN_PID 为本进程时 3 .. 3+LISTEN_FDS 是交给本进程的 fd，此前没有所有者；
        // 每个 fd 只接管一次（take_systemd 已清除环境变量，不会再次解析）。
        Ok(unsafe { OwnedFd::from_raw_fd(raw) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: u32 = 4242;

    fn parse(pid: Option<&str>, fds: Option<&str>, names: Option<&str>) -> Result<Option<ListenFds>, ActivationError> {
        parse_listen_env(ME, pid, fds, names)
    }

    #[test]
    fn listen_env_table() {
        let named = |n: &[&str]| Ok(Some(ListenFds { names: n.iter().map(|s| (*s).to_owned()).collect() }));
        /// （说明，LISTEN_PID，LISTEN_FDS，LISTEN_FDNAMES，期望：`Err(())` = 环境变量错误）
        type Case<'a> = (&'a str, Option<&'a str>, Option<&'a str>, Option<&'a str>, Result<Option<ListenFds>, ()>);
        let cases: Vec<Case> = vec![
            ("没有变量", None, None, None, Ok(None)),
            ("只有 LISTEN_FDS", None, Some("2"), None, Ok(None)),
            ("只有 LISTEN_PID", Some("4242"), None, None, Ok(None)),
            ("LISTEN_PID 是别的进程", Some("1"), Some("2"), None, Ok(None)),
            ("LISTEN_FDS=0", Some("4242"), Some("0"), None, Ok(None)),
            ("LISTEN_PID 不是数字", Some("abc"), Some("2"), None, Err(())),
            ("LISTEN_FDS 不是数字", Some("4242"), Some("two"), None, Err(())),
            ("LISTEN_FDS 为负", Some("4242"), Some("-1"), None, Err(())),
            ("超过上限", Some("4242"), Some("9"), None, Err(())),
        ];
        for (what, pid, fds, names, want) in cases {
            let got = parse(pid, fds, names);
            match want {
                Ok(v) => assert_eq!(got, Ok(v), "{what}"),
                Err(()) => assert!(matches!(got, Err(ActivationError::Env(_))), "{what}：{got:?}"),
            }
        }
        assert_eq!(parse(Some("4242"), Some("2"), Some("app-mcp-host:app-mcp-host")), named(&["app-mcp-host", "app-mcp-host"]));
        assert_eq!(parse(Some(" 4242 "), Some(" 1"), None), named(&["unknown"]), "缺少名字");
        assert_eq!(parse(Some("4242"), Some("2"), Some("only-one")), named(&["unknown", "unknown"]), "名字个数不符");
        assert_eq!(parse(Some("4242"), Some("8"), None).unwrap().unwrap().names.len(), MAX_LISTEN_FDS);
    }

    #[cfg(unix)]
    fn owned<T: Into<std::os::fd::OwnedFd>>(l: T) -> std::os::fd::OwnedFd {
        l.into()
    }

    #[cfg(unix)]
    #[test]
    fn classify_by_address_family() {
        let tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = tcp.local_addr().unwrap();
        let path = std::env::temp_dir().join(format!("amcp-act-{:016x}.sock", rand::random::<u64>()));
        let unix = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let got = assemble(Source::Systemd, vec![(owned(tcp), "a".into()), (owned(unix), "b".into())]).unwrap().unwrap();
        assert_eq!(got.listeners.tcp.as_ref().unwrap().local_addr().unwrap(), addr);
        assert_eq!(
            got.listeners.ipc.as_ref().unwrap().local_addr().unwrap().as_pathname(),
            Some(path.as_path())
        );
        assert_eq!(got.described, [format!("{addr}（a）"), format!("{}（b）", path.display())]);
        drop(got);
        let _ = std::fs::remove_file(&path);
        assert!(assemble(Source::Systemd, Vec::new()).unwrap().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_duplicate_and_non_listening_sockets() {
        let a = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let b = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let err = assemble(Source::Systemd, vec![(owned(a), "x".into()), (owned(b), "x".into())]).unwrap_err();
        assert!(matches!(&err, ActivationError::Socket(m) if m.contains("多于一个")), "{err}");

        // 已连接的流套接字（不是监听套接字）
        let (s, _peer) = std::os::unix::net::UnixStream::pair().unwrap();
        let err = assemble(Source::Systemd, vec![(owned(s), "x".into())]).unwrap_err();
        assert!(matches!(&err, ActivationError::Socket(m) if m.contains("不是监听")), "{err}");

        // 数据报套接字
        let udp = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        assert!(assemble(Source::Systemd, vec![(owned(udp), "x".into())]).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn closed_fd_is_reported() {
        // fd 1023 在测试进程中不会被打开（标准测试运行器远用不到）
        let err = sys::adopt_listen_fd(1023).unwrap_err();
        assert!(matches!(&err, ActivationError::Socket(m) if m.contains("未打开")), "{err}");
    }
}
