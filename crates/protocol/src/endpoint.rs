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
//! 默认端点（[`default_endpoint`]）：环境变量 [`ENDPOINT_ENV`] → 平台默认 IPC 端点
//! （[`default_ipc_endpoint`]）→ `ws://127.0.0.1:7717`（平台没有默认 IPC 端点时，如 Android / iOS）。
//! 这三步是**配置的解析顺序**，不是连接失败后的回退：选定的端点连不上时 SDK 按退避重连同一个端点。

use std::fmt;
use std::path::{Path, PathBuf};

/// 覆盖默认端点的环境变量（SDK 侧）。值为任一端点字符串。
pub const ENDPOINT_ENV: &str = "APP_MCP_ENDPOINT";

/// 本地 IPC 上 WebSocket 握手请求使用的 URL（Host 不检查路径与 Host 头）。
pub const IPC_WS_URL: &str = "ws://localhost/";

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
            if !path.starts_with('/') {
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
/// - Android / iOS / WASM：无（App 沙箱之间不能共享套接字；这些平台用 WebSocket）。
///
/// 无法确定位置（没有主目录、取不到 SID）时返回 `None`。
pub fn default_ipc_endpoint() -> Option<Endpoint> {
    platform_default()
}

#[cfg(all(unix, not(any(target_os = "android", target_os = "ios"))))]
fn platform_default() -> Option<Endpoint> {
    unix_default_path(
        std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).as_deref(),
        std::env::var_os("HOME").map(PathBuf::from).as_deref(),
    )
    .map(Endpoint::Unix)
}

#[cfg(windows)]
fn platform_default() -> Option<Endpoint> {
    win::current_user_sid()
        .ok()
        .map(|sid| Endpoint::Pipe(format!("{PIPE_NAME_PREFIX}{sid}")))
}

#[cfg(not(any(windows, all(unix, not(any(target_os = "android", target_os = "ios"))))))]
fn platform_default() -> Option<Endpoint> {
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
/// [`default_ipc_endpoint`] → `ws://127.0.0.1:7717`。
pub fn default_endpoint() -> String {
    match std::env::var(ENDPOINT_ENV) {
        Ok(v) if !v.is_empty() => v,
        _ => default_endpoint_without_env(),
    }
}

/// 不看环境变量的默认端点字符串。
pub fn default_endpoint_without_env() -> String {
    match default_ipc_endpoint() {
        Some(e) => e.to_string(),
        None => format!("ws://{}", crate::DEFAULT_WS_ADDR),
    }
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
    use super::*;

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
        #[cfg(all(unix, not(any(target_os = "android", target_os = "ios"))))]
        assert!(matches!(e, Endpoint::Unix(_)), "{s}");
        #[cfg(windows)]
        assert!(
            matches!(&e, Endpoint::Pipe(n) if n.starts_with(PIPE_NAME_PREFIX)),
            "{s}"
        );
        let _ = e;
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
