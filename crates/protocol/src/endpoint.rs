//! 传输端点（spec/protocol.md 第 1 节）：SDK 连接 Host 的地址格式、默认位置与本地 IPC 的身份辅助。
//!
//! 端点字符串有三种形式：
//!
//! - `ws://…` / `wss://…`：WebSocket over TCP（网页只能用这种；原生 App 显式配置时也可用）。
//! - `unix:<绝对路径>`：Unix 域套接字（Linux、macOS）。
//! - `pipe:\\.\pipe\<名称>`：Windows 命名管道。
//!
//! 本地 IPC 两种形式上跑的仍是同一套 WebSocket 帧与 JSON-RPC 消息（握手请求的 URL 固定为
//! [`IPC_WS_URL`]），与 TCP 上的协议逐字节相同。
//!
//! 默认端点（[`default_endpoint`]）：环境变量 [`ENDPOINT_ENV`] → 登记文件
//! （[`crate::registry`]，运行中的 Host 写下的实际端点）→ 平台默认 IPC 端点（[`default_ipc_endpoint`]）→
//! `ws://127.0.0.1:7717/app`（平台没有默认 IPC 端点时，如 Android / iOS / 鸿蒙，见 [`crate::platform`]）。
//! 这四步是**配置的解析顺序**，不是连接失败后的回退：选定的端点连不上时 SDK 按退避重连同一个端点。

use std::fmt;
use std::path::{Path, PathBuf};

use crate::platform::{IpcKind, Target};

/// 覆盖默认端点的环境变量（SDK 侧）。值为任一端点字符串。
pub const ENDPOINT_ENV: &str = "APP_MCP_ENDPOINT";

/// 本地 IPC 上 WebSocket 握手请求使用的 URL（Host 不检查 `Host` 头；路径与 TCP 相同为 `/app`）。
pub const IPC_WS_URL: &str = "ws://localhost/app";

/// Unix 域套接字的文件名。
pub const UNIX_SOCKET_NAME: &str = "hub.sock";

/// Windows 命名管道名前缀；完整名为 `\\.\pipe\app-mcp-<当前用户 SID>`。
pub const PIPE_NAME_PREFIX: &str = r"\\.\pipe\app-mcp-";

const PIPE_ROOT: &str = r"\\.\pipe\";

/// 一个已解析的端点。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Endpoint {
    /// `ws://` / `wss://` URL（原样保存）。
    WebSocket(String),
    /// Unix 域套接字的绝对路径。
    Unix(PathBuf),
    /// Windows 命名管道的完整名（`\\.\pipe\…`）。
    Pipe(String),
}

impl Endpoint {
    /// 解析端点字符串。不认识的形式返回错误说明（不做任何猜测或转换）。
    pub fn parse(s: &str) -> Result<Self, String> {
        let scheme = |p: &str| {
            s.len() > p.len() && s.get(..p.len()).is_some_and(|h| h.eq_ignore_ascii_case(p))
        };
        if scheme("ws://") || scheme("wss://") {
            return Ok(Self::WebSocket(s.to_owned()));
        }
        if let Some(path) = s.strip_prefix("unix:") {
            if !is_unix_absolute(path) {
                return Err(format!("unix: 端点必须是绝对路径：{s:?}"));
            }
            return Ok(Self::Unix(PathBuf::from(path)));
        }
        if let Some(name) = s.strip_prefix("pipe:") {
            let ok = name
                .get(..PIPE_ROOT.len())
                .is_some_and(|p| p.eq_ignore_ascii_case(PIPE_ROOT))
                && name.len() > PIPE_ROOT.len()
                && !name[PIPE_ROOT.len()..].contains('\\');
            if !ok {
                return Err(format!(r"pipe: 端点必须形如 pipe:\\.\pipe\<名称>：{s:?}"));
            }
            return Ok(Self::Pipe(name.to_owned()));
        }
        Err(format!(
            r"端点必须以 ws://、wss://、unix:<绝对路径> 或 pipe:\\.\pipe\<名称> 开头：{s:?}"
        ))
    }

    /// 是否为本地 IPC（Unix 域套接字 / 命名管道）。
    pub fn is_ipc(&self) -> bool {
        !matches!(self, Self::WebSocket(_))
    }

    /// Unix 域套接字端点。
    pub fn unix(path: impl Into<PathBuf>) -> Self {
        Self::Unix(path.into())
    }

    /// 连接本端点时的传输类别（spec/lifecycle.md 第 11 节），决定 SDK 是否需要心跳。
    ///
    /// - `unix:` / `pipe:` → [`TransportKind::Ipc`]；
    /// - 回环主机（[`is_loopback_host`]）的 `ws://` / `wss://`：桌面平台 → [`TransportKind::Loopback`]；
    ///   沙箱平台（Android / iOS / 鸿蒙，[`Target::is_app_sandboxed`]）→ [`TransportKind::Remote`]；
    /// - 其他主机 → [`TransportKind::Remote`]。
    ///
    /// @why 沙箱平台上的回环地址通常是 `adb reverse` / `hdc rport` 之类的转发（Host 在另一台机器上）：设备侧只看到
    /// 本机回环，实际跨 USB / 网络，USB 断开、电脑休眠等远端变化未经验证能否及时以 EOF 送达，按远程保守处理。
    pub fn transport_kind(&self, target: &Target<'_>) -> TransportKind {
        match self {
            Self::Unix(_) | Self::Pipe(_) => TransportKind::Ipc,
            Self::WebSocket(url) if url_host(url).is_some_and(is_loopback_host) && !target.is_app_sandboxed() => {
                TransportKind::Loopback
            }
            Self::WebSocket(_) => TransportKind::Remote,
        }
    }
}

/// SDK 与 Host 之间的传输类别（spec/lifecycle.md 第 11 节）。由驱动层（原生运行时 / 网页驱动层）按端点判定后
/// 告知核心（`ClientConfig::transport`），核心据此决定是否发心跳。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TransportKind {
    /// 驱动层未告知：按 [`TransportKind::Remote`] 处理（保守，发心跳）。
    #[default]
    Unknown,
    /// 本地 IPC（Unix 域套接字 / 命名管道）：对端退出时立即读到 EOF。
    Ipc,
    /// 本机回环 TCP（含网页经 SharedWorker 的共享连接）：对端退出时立即收到 RST / FIN。
    Loopback,
    /// 跨机器或经转发（含沙箱平台上的回环地址，如 `adb reverse`）：断开可能无法及时感知。
    Remote,
}

/// `ws://` / `wss://` URL 的主机部分（去掉用户信息、端口与 IPv6 方括号）；格式不对时为 `None`。
fn url_host(url: &str) -> Option<&str> {
    let (_, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    if let Some(v6) = host_port.strip_prefix('[') {
        return v6.split_once(']').map(|(h, _)| h);
    }
    Some(host_port.split_once(':').map_or(host_port, |(h, _)| h))
}

/// 主机名是否为本机回环：`localhost`（不区分大小写）、`127.0.0.0/8`、`::1`。
/// Unix 域套接字路径是否为绝对路径（POSIX 规则：以 `/` 开头）。
///
/// @why 不用 `Path::is_absolute`：它按编译平台判定，在 Windows 上检查 macOS / Linux 的套接字路径（doctor 核对登记、
/// 生成 launchd / systemd 服务文件）会把 `/run/…` 误判为相对路径。
pub fn is_unix_absolute(path: &str) -> bool {
    path.starts_with('/')
}

pub fn is_loopback_host(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost")
        || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WebSocket(url) => f.write_str(url),
            Self::Unix(path) => write!(f, "unix:{}", path.display()),
            Self::Pipe(name) => write!(f, "pipe:{name}"),
        }
    }
}

impl std::str::FromStr for Endpoint {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        Self::parse(s)
    }
}

/// 平台默认的本地 IPC 端点（Host 默认监听、SDK 默认连接的位置）。
///
/// - Linux：`$XDG_RUNTIME_DIR/app-mcp/hub.sock`；未设置 `XDG_RUNTIME_DIR` 时 `~/.app-mcp/run/hub.sock`。
/// - macOS 等其他 Unix：`~/.app-mcp/run/hub.sock`（设置了 `XDG_RUNTIME_DIR` 时同 Linux）。
/// - Windows：`\\.\pipe\app-mcp-<当前用户 SID>`。
/// - Android / iOS / 鸿蒙（`target_env = "ohos"`）/ WASM：无（App 沙箱之间不能共享套接字；这些平台用 WebSocket）。
///   平台分类见 [`crate::platform::Target::default_ipc_kind`]。
///
/// 无法确定位置（没有主目录、取不到 SID）时返回 `None`。
pub fn default_ipc_endpoint() -> Option<Endpoint> {
    match Target::CURRENT.default_ipc_kind()? {
        IpcKind::Unix => unix_default_from_env(),
        IpcKind::Pipe => windows_default_pipe(),
    }
}

fn unix_default_from_env() -> Option<Endpoint> {
    unix_default_path(
        std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).as_deref(),
        std::env::var_os("HOME").map(PathBuf::from).as_deref(),
    )
    .map(Endpoint::Unix)
}

#[cfg(windows)]
fn windows_default_pipe() -> Option<Endpoint> {
    win::current_user_sid()
        .ok()
        .map(|sid| Endpoint::Pipe(format!("{PIPE_NAME_PREFIX}{sid}")))
}

#[cfg(not(windows))]
fn windows_default_pipe() -> Option<Endpoint> {
    None
}

/// Unix 默认路径的计算（纯函数，便于测试）：运行时目录优先，其次主目录。只接受绝对路径。
pub fn unix_default_path(runtime_dir: Option<&Path>, home: Option<&Path>) -> Option<PathBuf> {
    if let Some(dir) = runtime_dir.filter(|d| d.is_absolute()) {
        return Some(dir.join("app-mcp").join(UNIX_SOCKET_NAME));
    }
    home.filter(|h| h.is_absolute())
        .map(|h| h.join(".app-mcp").join("run").join(UNIX_SOCKET_NAME))
}

/// SDK 的默认端点字符串：环境变量 [`ENDPOINT_ENV`]（非空时原样使用，由调用方校验）→
/// 登记文件中的端点（[`crate::registry::registered_app_endpoint`]）→ [`default_ipc_endpoint`] →
/// `ws://127.0.0.1:7717/app`。
pub fn default_endpoint() -> String {
    match std::env::var(ENDPOINT_ENV) {
        Ok(v) if !v.is_empty() => v,
        _ => crate::registry::registered_app_endpoint().unwrap_or_else(default_endpoint_without_env),
    }
}

/// 不看环境变量与登记文件的默认端点字符串：平台默认 IPC 端点，没有时为 `ws://127.0.0.1:7717/app`。
pub fn default_endpoint_without_env() -> String {
    match default_ipc_endpoint() {
        Some(e) => e.to_string(),
        None => crate::DEFAULT_WS_URL.to_owned(),
    }
}

/// Unix 域套接字路径的最大字节数：`sockaddr_un.sun_path` 的长度减去结尾 NUL（Linux / Android 107，macOS / BSD 103）。
#[cfg(unix)]
pub const MAX_UNIX_SOCKET_PATH_BYTES: usize =
    std::mem::size_of::<libc::sockaddr_un>() - std::mem::offset_of!(libc::sockaddr_un, sun_path) - 1;

/// 检查 Unix 域套接字路径能否放进 `sockaddr_un`（Hub 绑定前、SDK 连接前调用）。
///
/// @error 超过 [`MAX_UNIX_SOCKET_PATH_BYTES`] 时返回 `IPC_PATH_TOO_LONG`，说明中带实际长度、上限与修复建议
/// （spec/protocol.md 10.1）。
#[cfg(unix)]
pub fn check_unix_socket_path(path: &Path) -> Result<(), crate::diagnostic::ConnectionIssue> {
    use crate::diagnostic::{ConnectionErrorCode, ConnectionIssue};
    let len = path.as_os_str().len();
    if len <= MAX_UNIX_SOCKET_PATH_BYTES {
        return Ok(());
    }
    let code = ConnectionErrorCode::IpcPathTooLong;
    Err(ConnectionIssue::new(
        code,
        format!(
            "本地 IPC 套接字路径过长（{len} 字节，本平台上限 {MAX_UNIX_SOCKET_PATH_BYTES} 字节）：{}。建议：{}",
            path.display(),
            code.hint()
        ),
    ))
}

/// Windows 命名管道完整名（`\\.\pipe\<名称>`，含前缀）的最大字符数（UTF-16 码元）：
/// `CreateNamedPipeW` 文档规定整个管道名最长 256 字符。
pub const MAX_PIPE_NAME_CHARS: usize = 256;

/// 检查 Windows 命名管道完整名的长度（Hub 创建管道前调用；纯函数，各平台可用）。
///
/// @input `name` 为完整名（`\\.\pipe\…`，即 [`Endpoint::Pipe`] 的内容，不含 `pipe:`）
/// @error 超过 [`MAX_PIPE_NAME_CHARS`] 时返回 `IPC_PATH_TOO_LONG`，说明中带实际长度、上限与修复建议
/// （spec/protocol.md 10.1）。
pub fn check_pipe_name(name: &str) -> Result<(), crate::diagnostic::ConnectionIssue> {
    use crate::diagnostic::{ConnectionErrorCode, ConnectionIssue};
    let len = name.encode_utf16().count();
    if len <= MAX_PIPE_NAME_CHARS {
        return Ok(());
    }
    let code = ConnectionErrorCode::IpcPathTooLong;
    Err(ConnectionIssue::new(
        code,
        format!(
            "命名管道名过长（{len} 字符，上限 {MAX_PIPE_NAME_CHARS} 字符）：{name}。建议：{}",
            code.hint()
        ),
    ))
}

/// 当前进程的有效用户 ID（Unix 域套接字的对端凭据检查用）。
#[cfg(unix)]
pub fn current_uid() -> u32 {
    // SAFETY: geteuid 没有前置条件，总是成功。
    unsafe { libc::geteuid() }
}

/// Windows 命名管道的身份与安全描述符辅助。
#[cfg(windows)]
pub mod win {
    use std::ffi::c_void;
    use std::io;
    use std::os::windows::io::RawHandle;

    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, HANDLE, HLOCAL, LocalFree};
    use windows_sys::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        GetSecurityInfo, SDDL_REVISION_1, SE_KERNEL_OBJECT,
    };
    use windows_sys::Win32::Security::{
        GetTokenInformation, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
        SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
    };
    use windows_sys::Win32::System::Pipes::GetNamedPipeClientProcessId;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    /// `LocalAlloc` 分配的内存，Drop 时 `LocalFree`。
    struct Local(*mut c_void);

    impl Drop for Local {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: 指针来自系统函数的 LocalAlloc 分配，只释放一次。
                unsafe { LocalFree(self.0 as HLOCAL) };
            }
        }
    }

    /// SID → 字符串（`S-1-5-21-…`）。
    ///
    /// # Safety
    /// `sid` 必须指向有效的 SID。
    unsafe fn sid_to_string(sid: PSID) -> io::Result<String> {
        let mut out: *mut u16 = std::ptr::null_mut();
        // SAFETY: 调用方保证 sid 有效；out 由系统分配，交给 Local 释放。
        if unsafe { ConvertSidToStringSidW(sid, &mut out) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let _guard = Local(out.cast());
        let mut len = 0;
        // SAFETY: out 是以 0 结尾的 UTF-16 字符串。
        while unsafe { *out.add(len) } != 0 {
            len += 1;
        }
        // SAFETY: 上面数出的长度在分配范围内。
        let wide = unsafe { std::slice::from_raw_parts(out, len) };
        Ok(String::from_utf16_lossy(wide))
    }

    /// 当前进程令牌的用户 SID（字符串形式）。
    pub fn current_user_sid() -> io::Result<String> {
        let mut token: HANDLE = std::ptr::null_mut();
        // SAFETY: 伪句柄 GetCurrentProcess 总是有效；token 成功后由下方关闭。
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let result = (|| {
            let mut len = 0u32;
            // SAFETY: 第一次调用只取所需长度。
            unsafe { GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut len) };
            if len == 0 {
                return Err(io::Error::last_os_error());
            }
            // 以 u64 为单位分配，保证 TOKEN_USER 的对齐。
            let mut buf = vec![0u64; (len as usize).div_ceil(8)];
            // SAFETY: buf 至少 len 字节。
            if unsafe {
                GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), len, &mut len)
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: 成功时 buf 开头是 TOKEN_USER，其中的 SID 指针指向 buf 内部。
            let user = unsafe { &*(buf.as_ptr().cast::<TOKEN_USER>()) };
            // SAFETY: 同上。
            unsafe { sid_to_string(user.User.Sid) }
        })();
        // SAFETY: token 是上面打开的句柄。
        unsafe { CloseHandle(token) };
        result
    }

    /// 命名管道（或其他内核对象）句柄的所有者 SID（字符串形式）。句柄需有 `READ_CONTROL` 权限
    /// （以读写方式打开的管道客户端具备）。
    pub fn handle_owner_sid(handle: RawHandle) -> io::Result<String> {
        let mut owner: PSID = std::ptr::null_mut();
        let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        // SAFETY: handle 由调用方保证有效；sd 由系统分配，交给 Local 释放。
        let rc = unsafe {
            GetSecurityInfo(
                handle as HANDLE,
                SE_KERNEL_OBJECT,
                OWNER_SECURITY_INFORMATION,
                &mut owner,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut sd,
            )
        };
        if rc != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(rc as i32));
        }
        let _guard = Local(sd);
        if owner.is_null() {
            return Err(io::Error::other("对象没有所有者"));
        }
        // SAFETY: owner 指向 sd 内部，sd 在 _guard 释放前有效。
        unsafe { sid_to_string(owner) }
    }

    /// 管道服务端句柄上已连接客户端的进程 ID。
    pub fn pipe_client_pid(handle: RawHandle) -> Option<u32> {
        let mut pid = 0u32;
        // SAFETY: handle 由调用方保证是命名管道服务端句柄。
        (unsafe { GetNamedPipeClientProcessId(handle as HANDLE, &mut pid) } != 0).then_some(pid)
    }

    /// 只允许当前用户访问、所有者为当前用户的安全描述符（SDDL `O:<sid>D:P(A;;GA;;;<sid>)`），
    /// 用于创建命名管道。
    pub struct PipeSecurity {
        sd: Local,
    }

    // SAFETY: 安全描述符创建后只读，可以跨线程共享。
    unsafe impl Send for PipeSecurity {}
    // SAFETY: 同上。
    unsafe impl Sync for PipeSecurity {}

    impl PipeSecurity {
        /// 为当前用户创建。
        pub fn current_user() -> io::Result<Self> {
            let sid = current_user_sid()?;
            let sddl: Vec<u16> = format!("O:{sid}D:P(A;;GA;;;{sid})")
                .encode_utf16()
                .chain(Some(0))
                .collect();
            let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
            // SAFETY: sddl 以 0 结尾；sd 由系统分配，交给 Local 释放。
            if unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    sddl.as_ptr(),
                    SDDL_REVISION_1,
                    &mut sd,
                    std::ptr::null_mut(),
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(Self { sd: Local(sd) })
        }

        /// 以 `SECURITY_ATTRIBUTES` 指针调用 `f`（指针只在 `f` 执行期间有效）。
        pub fn with_attributes<R>(&self, f: impl FnOnce(*mut c_void) -> R) -> R {
            let mut sa = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: self.sd.0,
                bInheritHandle: 0,
            };
            f((&mut sa as *mut SECURITY_ATTRIBUTES).cast())
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn transport_kinds() {
        use super::{Endpoint, TransportKind as K, is_loopback_host};
        use crate::platform::Target;
        let desktop = Target { family: "unix", os: "linux", env: "gnu" };
        let android = Target { family: "unix", os: "android", env: "" };
        let ohos = Target { family: "unix", os: "linux", env: "ohos" };
        let cases = [
            ("unix:/run/app-mcp/hub.sock", K::Ipc, K::Ipc),
            (r"pipe:\\.\pipe\app-mcp-x", K::Ipc, K::Ipc),
            ("ws://127.0.0.1:7717/app", K::Loopback, K::Remote),
            ("ws://localhost:7717/app", K::Loopback, K::Remote),
            ("ws://[::1]:7717/app", K::Loopback, K::Remote),
            ("wss://user@127.0.0.2/app?x=1", K::Loopback, K::Remote),
            ("ws://192.168.1.5:7717/app", K::Remote, K::Remote),
            ("wss://example.com/app", K::Remote, K::Remote),
            ("ws://127.0.0.1.example.com/app", K::Remote, K::Remote),
        ];
        for (text, on_desktop, on_mobile) in cases {
            let e = Endpoint::parse(text).unwrap();
            assert_eq!(e.transport_kind(&desktop), on_desktop, "{text}");
            assert_eq!(e.transport_kind(&android), on_mobile, "{text}");
            assert_eq!(e.transport_kind(&ohos), on_mobile, "{text}");
        }
        assert!(is_loopback_host("LocalHost") && is_loopback_host("127.9.9.9") && is_loopback_host("[::1]"));
        assert!(!is_loopback_host("10.0.0.1") && !is_loopback_host("localhost.example"));
    }


    /// 上限与标准库构造 `sockaddr_un` 的判定一致；超长路径给出 `IPC_PATH_TOO_LONG`。
    #[cfg(unix)]
    #[test]
    fn unix_socket_path_limit() {
        use crate::diagnostic::ConnectionErrorCode;
        use std::os::unix::net::SocketAddr;
        let path_of = |n: usize| PathBuf::from(format!("/{}", "a".repeat(n - 1)));
        let max = MAX_UNIX_SOCKET_PATH_BYTES;
        assert!(max >= 100, "{max}");
        assert!(SocketAddr::from_pathname(path_of(max)).is_ok());
        assert!(SocketAddr::from_pathname(path_of(max + 1)).is_err());
        assert_eq!(check_unix_socket_path(&path_of(max)), Ok(()));
        let issue = check_unix_socket_path(&path_of(max + 1)).unwrap_err();
        assert_eq!(issue.code, ConnectionErrorCode::IpcPathTooLong);
        assert!(issue.message.contains(&format!("{} 字节", max + 1)), "{issue}");
        assert!(issue.message.contains("--ipc-endpoint"), "{issue}");
    }

    use super::*;

    /// 命名管道完整名按 UTF-16 码元计数，上限 256；超出给出 `IPC_PATH_TOO_LONG`。
    #[test]
    fn pipe_name_limit() {
        use crate::diagnostic::ConnectionErrorCode;
        let name_of = |n: usize| format!("{PIPE_ROOT}{}", "a".repeat(n - PIPE_ROOT.len()));
        let max = MAX_PIPE_NAME_CHARS;
        assert_eq!(check_pipe_name(&name_of(max)), Ok(()));
        assert_eq!(check_pipe_name(&format!("{PIPE_NAME_PREFIX}S-1-5-21-1-2-3-1001")), Ok(()));
        let issue = check_pipe_name(&name_of(max + 1)).unwrap_err();
        assert_eq!(issue.code, ConnectionErrorCode::IpcPathTooLong);
        assert!(issue.message.contains(&format!("{} 字符", max + 1)), "{issue}");
        assert!(issue.message.contains("--ipc-endpoint"), "{issue}");
        // 非 ASCII：按 UTF-16 码元计（BMP 外字符占 2 个）。
        let wide = format!("{PIPE_ROOT}{}", "😀".repeat((max - PIPE_ROOT.len()) / 2));
        assert_eq!(check_pipe_name(&wide), Ok(()));
        assert!(check_pipe_name(&format!("{wide}😀")).is_err());
    }

    /// 按 POSIX 规则判定，与编译平台无关（回归：Windows 上 `Path::is_absolute("/run/…")` 为假）。
    #[test]
    fn unix_absolute_is_platform_independent() {
        assert!(is_unix_absolute("/run/user/1/app-mcp/hub.sock"));
        assert!(is_unix_absolute("/Users/u/.app-mcp/run/apps/my-shop.sock"));
        for rel in ["my-shop.sock", "run/hub.sock", "", r"C:\x\hub.sock"] {
            assert!(!is_unix_absolute(rel), "{rel}");
        }
    }

    #[test]
    fn parses_all_forms() {
        assert_eq!(
            Endpoint::parse("ws://127.0.0.1:7717"),
            Ok(Endpoint::WebSocket("ws://127.0.0.1:7717".into()))
        );
        assert_eq!(
            Endpoint::parse("WSS://example.invalid"),
            Ok(Endpoint::WebSocket("WSS://example.invalid".into()))
        );
        assert_eq!(
            Endpoint::parse("unix:/run/user/1000/app-mcp/hub.sock"),
            Ok(Endpoint::Unix("/run/user/1000/app-mcp/hub.sock".into()))
        );
        assert_eq!(
            Endpoint::parse(r"pipe:\\.\pipe\app-mcp-S-1-5-21-1"),
            Ok(Endpoint::Pipe(r"\\.\pipe\app-mcp-S-1-5-21-1".into()))
        );
        for bad in [
            "",
            "ws://",
            "wss://",
            "http://127.0.0.1:1",
            "127.0.0.1:7717",
            "unix:relative.sock",
            "unix:",
            "pipe:app-mcp",
            r"pipe:\\.\pipe\",
            r"pipe:\\.\pipe\a\b",
            r"pipe:\\server\pipe\x",
        ] {
            assert!(Endpoint::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn display_round_trips() {
        for s in [
            "ws://127.0.0.1:7717",
            "unix:/tmp/a b/hub.sock",
            r"pipe:\\.\pipe\app-mcp-x",
        ] {
            let e = Endpoint::parse(s).unwrap();
            assert_eq!(e.to_string(), s);
            assert_eq!(Endpoint::parse(&e.to_string()).unwrap(), e);
        }
        assert!(Endpoint::parse("unix:/x").unwrap().is_ipc());
        assert!(!Endpoint::parse("ws://x").unwrap().is_ipc());
    }

    #[cfg(unix)]
    #[test]
    fn unix_default_prefers_runtime_dir() {
        assert_eq!(
            unix_default_path(Some(Path::new("/run/user/1000")), Some(Path::new("/home/u"))),
            Some(PathBuf::from("/run/user/1000/app-mcp/hub.sock"))
        );
        assert_eq!(
            unix_default_path(None, Some(Path::new("/home/u"))),
            Some(PathBuf::from("/home/u/.app-mcp/run/hub.sock"))
        );
        // 相对路径不可信：忽略。
        assert_eq!(
            unix_default_path(Some(Path::new("run")), Some(Path::new("/home/u"))),
            Some(PathBuf::from("/home/u/.app-mcp/run/hub.sock"))
        );
        assert_eq!(unix_default_path(None, Some(Path::new("home"))), None);
        assert_eq!(unix_default_path(None, None), None);
    }

    #[test]
    fn default_endpoint_is_parseable() {
        let s = default_endpoint_without_env();
        let e = Endpoint::parse(&s).unwrap();
        match Target::CURRENT.default_ipc_kind() {
            Some(IpcKind::Unix) => assert!(matches!(e, Endpoint::Unix(_)), "{s}"),
            Some(IpcKind::Pipe) => assert!(
                matches!(&e, Endpoint::Pipe(n) if n.starts_with(PIPE_NAME_PREFIX)),
                "{s}"
            ),
            None => assert_eq!(s, crate::DEFAULT_WS_URL),
        }
    }

    /// 沙箱平台（Android / iOS / 鸿蒙）没有默认 IPC 端点，默认端点为回环 WebSocket。
    #[cfg(any(target_os = "android", target_os = "ios", target_env = "ohos"))]
    #[test]
    fn sandboxed_default_is_loopback_websocket() {
        assert_eq!(default_ipc_endpoint(), None);
        assert_eq!(default_endpoint_without_env(), crate::DEFAULT_WS_URL);
        assert_eq!(crate::registry::registered_app_endpoint(), None);
    }

    #[cfg(windows)]
    #[test]
    fn windows_sid_and_security() {
        let sid = win::current_user_sid().unwrap();
        assert!(sid.starts_with("S-1-"), "{sid}");
        let sec = win::PipeSecurity::current_user().unwrap();
        assert!(sec.with_attributes(|p| !p.is_null()));
    }
}
