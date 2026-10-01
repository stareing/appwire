//! 本机端口与进程信息（`doctor`、`service install`、`serve` 端口被占用时说明占用者）：
//!
//! - [`listening_owner`]：哪个进程在监听某个 TCP 端口。Linux 读 `/proc/net/tcp{,6}` 取监听套接字的 inode，
//!   再在 `/proc/<pid>/fd` 中找到持有它的进程（其他用户的进程读不到 fd，此时只给出 uid）；Windows 用
//!   `GetExtendedTcpTable(TCP_TABLE_OWNER_PID_LISTENER)` 与 `QueryFullProcessImageNameW`；macOS 尽力而为，
//!   执行 `lsof -nP -iTCP:<port> -sTCP:LISTEN -Fpc`。
//! - [`excluded_port_ranges`]：Windows 的 TCP 排除端口段（Hyper-V / WinNAT 等保留，落在其中的端口无法绑定），
//!   解析 `netsh int ipv4 show excludedportrange protocol=tcp` 的输出。
//! - [`process_name`] / [`process_alive`]：按进程号取进程名、判断进程是否存在。
//!
//! 解析函数与平台无关，单独测试。

use std::io;

/// 监听某端口的进程。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortOwner {
    /// 进程号；Linux 上属于其他用户的进程读不到时为 `None`。
    pub pid: Option<u32>,
    /// 进程名（可执行文件名）。
    pub name: Option<String>,
    /// 监听套接字所属用户（Linux：uid）。
    pub uid: Option<u32>,
}

impl std::fmt::Display for PortOwner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.pid, &self.name) {
            (Some(pid), Some(name)) => write!(f, "{name}（pid {pid}）"),
            (Some(pid), None) => write!(f, "pid {pid}"),
            (None, _) => match self.uid {
                Some(uid) => write!(f, "uid {uid} 的进程（无权查看进程号）"),
                None => f.write_str("未知进程"),
            },
        }
    }
}

// ---------------------------------------------------------------------------
// 解析（与平台无关）
// ---------------------------------------------------------------------------

/// `/proc/net/tcp` / `tcp6` 中监听 `port` 的套接字：`(inode, uid)`。
pub fn parse_proc_net_tcp(text: &str, port: u16) -> Vec<(u64, u32)> {
    const LISTEN: &str = "0A";
    text.lines()
        .skip(1)
        .filter_map(|line| {
            let f: Vec<&str> = line.split_whitespace().collect();
            let local = f.get(1)?;
            let (_, p) = local.rsplit_once(':')?;
            let p = u16::from_str_radix(p, 16).ok()?;
            if p != port || *f.get(3)? != LISTEN {
                return None;
            }
            let uid = f.get(7)?.parse().ok()?;
            let inode = f.get(9)?.parse().ok()?;
            (inode != 0).then_some((inode, uid))
        })
        .collect()
}

/// `lsof -F pc` 的输出：第一个进程（`p<pid>` 与随后的 `c<命令名>`）。
pub fn parse_lsof(text: &str) -> Option<PortOwner> {
    let mut pid = None;
    let mut name = None;
    for line in text.lines() {
        if let Some(p) = line.strip_prefix('p') {
            if pid.is_some() {
                break;
            }
            pid = p.trim().parse().ok();
        } else if let Some(c) = line.strip_prefix('c')
            && pid.is_some()
            && name.is_none()
        {
            name = Some(c.trim().to_owned());
        }
    }
    pid.map(|pid| PortOwner { pid: Some(pid), name, uid: None })
}

/// Windows 排除端口段。
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExcludedRange {
    pub start: u16,
    pub end: u16,
    /// 管理员配置的排除（`netsh ... add excludedportrange`，行尾带 `*`）。
    pub administered: bool,
}

impl ExcludedRange {
    pub fn contains(&self, port: u16) -> bool {
        (self.start..=self.end).contains(&port)
    }
}

/// 解析 `netsh int ipv4 show excludedportrange protocol=tcp` 的输出：数据行为"起始端口 结束端口 [*]"，
/// 其余（标题、分隔线、`* - Administered port exclusions.` 说明，及本地化的同类文字）忽略。
pub fn parse_excluded_ranges(text: &str) -> Vec<ExcludedRange> {
    text.lines()
        .filter_map(|line| {
            let mut it = line.split_whitespace();
            let start = it.next()?.parse().ok()?;
            let end = it.next()?.parse().ok()?;
            let administered = match it.next() {
                None => false,
                Some("*") => true,
                Some(_) => return None,
            };
            (start <= end).then_some(ExcludedRange { start, end, administered })
        })
        .collect()
}

/// `adb reverse --list` 的输出中是否有把设备上 `tcp:<port>` 转到本机 `tcp:<port>` 的规则。
/// 每行形如 `<序列号> tcp:7717 tcp:7717`（旧版本前面可能有 `(reverse)`）。
pub fn adb_reverse_has(text: &str, device_port: u16, host_port: u16) -> bool {
    let want_remote = format!("tcp:{device_port}");
    let want_local = format!("tcp:{host_port}");
    text.lines().any(|line| {
        let f: Vec<&str> = line.split_whitespace().collect();
        f.windows(2).any(|w| w[0] == want_remote && w[1] == want_local)
    })
}

// ---------------------------------------------------------------------------
// 平台实现
// ---------------------------------------------------------------------------

/// 监听 `port`（任一地址、IPv4 或 IPv6）的进程；没有监听者时为 `Ok(None)`。
#[cfg(target_os = "linux")]
pub fn listening_owner(port: u16) -> io::Result<Option<PortOwner>> {
    let mut sockets = Vec::new();
    for table in ["/proc/net/tcp", "/proc/net/tcp6"] {
        match std::fs::read_to_string(table) {
            Ok(text) => sockets.extend(parse_proc_net_tcp(&text, port)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    let Some(&(_, uid)) = sockets.first() else {
        return Ok(None);
    };
    let wanted: Vec<String> = sockets.iter().map(|(inode, _)| format!("socket:[{inode}]")).collect();
    for entry in std::fs::read_dir("/proc")?.flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        // 其他用户的进程读不到 fd 目录：跳过。
        let Ok(fds) = std::fs::read_dir(entry.path().join("fd")) else {
            continue;
        };
        for fd in fds.flatten() {
            if let Ok(target) = std::fs::read_link(fd.path())
                && wanted.iter().any(|w| target.as_os_str() == w.as_str())
            {
                return Ok(Some(PortOwner { pid: Some(pid), name: process_name(pid), uid: Some(uid) }));
            }
        }
    }
    Ok(Some(PortOwner { pid: None, name: None, uid: Some(uid) }))
}

#[cfg(target_os = "macos")]
pub fn listening_owner(port: u16) -> io::Result<Option<PortOwner>> {
    let out = std::process::Command::new("lsof")
        .args(["-nP", &format!("-iTCP:{port}"), "-sTCP:LISTEN", "-Fpc"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()?;
    // 没有匹配时 lsof 退出码为 1、无输出。
    Ok(parse_lsof(&String::from_utf8_lossy(&out.stdout)))
}

#[cfg(windows)]
pub fn listening_owner(port: u16) -> io::Result<Option<PortOwner>> {
    Ok(win::listener_pids(port)?
        .into_iter()
        .next()
        .map(|pid| PortOwner { pid: Some(pid), name: process_name(pid), uid: None }))
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub fn listening_owner(_port: u16) -> io::Result<Option<PortOwner>> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "本平台不支持查询端口占用进程"))
}

/// Windows 的 TCP 排除端口段；其他平台为空。
pub fn excluded_port_ranges() -> io::Result<Vec<ExcludedRange>> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let out = std::process::Command::new("netsh")
            .args(["int", "ipv4", "show", "excludedportrange", "protocol=tcp"])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .output()?;
        if !out.status.success() {
            return Err(io::Error::other(format!("netsh 退出码 {}", out.status)));
        }
        Ok(parse_excluded_ranges(&String::from_utf8_lossy(&out.stdout)))
    }
    #[cfg(not(windows))]
    {
        Ok(Vec::new())
    }
}

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 进程名（可执行文件名，不含目录）。
pub fn process_name(pid: u32) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string(format!("/proc/{pid}/comm")).ok().map(|s| s.trim().to_owned())
    }
    #[cfg(windows)]
    {
        win::process_image(pid)
            .map(|p| p.rsplit(['\\', '/']).next().unwrap_or(&p).to_owned())
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        let out = std::process::Command::new("ps")
            .args(["-o", "comm=", "-p", &pid.to_string()])
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        let name = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        (!name.is_empty()).then(|| name.rsplit('/').next().unwrap_or(&name).to_owned())
    }
}

/// 进程是否存在。
pub fn process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        let Ok(pid) = i32::try_from(pid) else { return false };
        // SAFETY: kill(pid, 0) 只检查进程是否存在与权限，不发送信号。
        let r = unsafe { libc::kill(pid, 0) };
        r == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(windows)]
    {
        win::process_alive(pid)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        false
    }
}

#[cfg(windows)]
mod win {
    use std::io;

    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_INSUFFICIENT_BUFFER, NO_ERROR, STILL_ACTIVE};
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetExtendedTcpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCPROW_OWNER_PID, TCP_TABLE_OWNER_PID_LISTENER,
    };
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    };

    const AF_INET: u32 = 2;
    const AF_INET6: u32 = 23;

    /// 读取一张监听表（IPv4 / IPv6），返回原始字节（开头为 `dwNumEntries`）。
    fn table(family: u32) -> io::Result<Vec<u8>> {
        let mut size: u32 = 0;
        let mut buf: Vec<u8> = Vec::new();
        for _ in 0..4 {
            // SAFETY: buf 至少 size 字节（首次为空指针 + 0，用于取所需大小）。
            let ptr = if buf.is_empty() { std::ptr::null_mut() } else { buf.as_mut_ptr().cast() };
            let r = unsafe { GetExtendedTcpTable(ptr, &mut size, 0, family, TCP_TABLE_OWNER_PID_LISTENER, 0) };
            match r {
                NO_ERROR if !buf.is_empty() => return Ok(buf),
                NO_ERROR => return Ok(vec![0; 4]),
                ERROR_INSUFFICIENT_BUFFER => buf = vec![0u8; size as usize + 256],
                e => return Err(io::Error::from_raw_os_error(e as i32)),
            }
        }
        Err(io::Error::other("GetExtendedTcpTable：缓冲区大小不断变化"))
    }

    /// 解析表中的行：`(本地端口, 进程号)`。行为 `#[repr(C)]` 的定长结构，紧跟在 4 字节计数之后。
    fn rows<R: Copy>(buf: &[u8], port_pid: impl Fn(&R) -> (u32, u32)) -> Vec<(u16, u32)> {
        let Some(count) = buf.get(..4).map(|b| u32::from_ne_bytes([b[0], b[1], b[2], b[3]]) as usize) else {
            return Vec::new();
        };
        let row = std::mem::size_of::<R>();
        let align = std::mem::align_of::<R>().max(4);
        let start = 4usize.next_multiple_of(align);
        (0..count)
            .filter_map(|i| {
                let off = start + i * row;
                let bytes = buf.get(off..off + row)?;
                // SAFETY: bytes 长度为 size_of::<R>()；R 是只含整数 / 字节数组的 repr(C) 结构，任意位模式有效；
                // read_unaligned 不要求对齐。
                let r: R = unsafe { std::ptr::read_unaligned(bytes.as_ptr().cast()) };
                let (port, pid) = port_pid(&r);
                // dwLocalPort 的低 16 位为网络字节序的端口。
                Some((u16::from_be_bytes([(port & 0xff) as u8, ((port >> 8) & 0xff) as u8]), pid))
            })
            .collect()
    }

    pub fn listener_pids(port: u16) -> io::Result<Vec<u32>> {
        let mut out: Vec<u32> = rows::<MIB_TCPROW_OWNER_PID>(&table(AF_INET)?, |r| (r.dwLocalPort, r.dwOwningPid))
            .into_iter()
            .chain(rows::<MIB_TCP6ROW_OWNER_PID>(&table(AF_INET6)?, |r| (r.dwLocalPort, r.dwOwningPid)))
            .filter(|(p, _)| *p == port)
            .map(|(_, pid)| pid)
            .collect();
        out.dedup();
        Ok(out)
    }

    fn with_process<T>(pid: u32, f: impl FnOnce(windows_sys::Win32::Foundation::HANDLE) -> Option<T>) -> Option<T> {
        // SAFETY: 普通的 Win32 调用；句柄在本函数内关闭。
        let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if h.is_null() {
            return None;
        }
        let r = f(h);
        // SAFETY: h 是 OpenProcess 返回的有效句柄。
        unsafe { CloseHandle(h) };
        r
    }

    pub fn process_image(pid: u32) -> Option<String> {
        with_process(pid, |h| {
            let mut buf = vec![0u16; 1024];
            let mut len = buf.len() as u32;
            // SAFETY: buf 有 len 个 u16；成功时 len 为写入的字符数。
            let ok = unsafe { QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) };
            (ok != 0).then(|| String::from_utf16_lossy(&buf[..len as usize]))
        })
    }

    pub fn process_alive(pid: u32) -> bool {
        with_process(pid, |h| {
            let mut code = 0u32;
            // SAFETY: h 有效，code 可写。
            let ok = unsafe { GetExitCodeProcess(h, &mut code) };
            Some(ok != 0 && code == STILL_ACTIVE as u32)
        })
        .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proc_net_tcp() {
        let text = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 0100007F:1E25 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 123456 1 0000000000000000 100 0 0 10 0
   1: 0100007F:1E25 0100007F:C350 01 00000000:00000000 00:00000000 00000000  1000        0 0 1 0000000000000000 20 4 30 10 -1
   2: 00000000:0016 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 999 1 0000000000000000 100 0 0 10 0
";
        // 0x1E25 = 7717；ESTABLISHED（01）不算
        assert_eq!(parse_proc_net_tcp(text, 7717), vec![(123456, 1000)]);
        assert_eq!(parse_proc_net_tcp(text, 22), vec![(999, 0)]);
        assert!(parse_proc_net_tcp(text, 7737).is_empty());
        let v6 = "  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 00000000000000000000000001000000:1E39 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1001        0 42 1 0 100 0 0 10 0
";
        assert_eq!(parse_proc_net_tcp(v6, 7737), vec![(42, 1001)]);
    }

    #[test]
    fn lsof_output() {
        assert_eq!(
            parse_lsof("p4242\ncnode\nf23\np99\ncother\n"),
            Some(PortOwner { pid: Some(4242), name: Some("node".into()), uid: None })
        );
        assert_eq!(parse_lsof(""), None);
    }

    #[test]
    fn netsh_output() {
        let text = "
Protocol tcp Port Exclusion Ranges

Start Port    End Port
----------    --------
      5357        5357
      7700        7799
     50000       50059     *

* - Administered port exclusions.
";
        let r = parse_excluded_ranges(text);
        assert_eq!(
            r,
            vec![
                ExcludedRange { start: 5357, end: 5357, administered: false },
                ExcludedRange { start: 7700, end: 7799, administered: false },
                ExcludedRange { start: 50000, end: 50059, administered: true },
            ]
        );
        assert!(r[1].contains(7717) && !r[0].contains(7717));
        // 本地化输出（中文 Windows）同样只取数据行
        let zh = "协议 tcp 端口排除范围\n\n开始端口    结束端口\n----------    --------\n      7717        7717\n\n* - 管理的端口排除。\n";
        assert_eq!(parse_excluded_ranges(zh), vec![ExcludedRange { start: 7717, end: 7717, administered: false }]);
    }

    #[test]
    fn adb_reverse_list() {
        let text = "emulator-5554 tcp:7717 tcp:7717\n(reverse) tcp:8081 tcp:8081\n";
        assert!(adb_reverse_has(text, 7717, 7717));
        assert!(adb_reverse_has(text, 8081, 8081));
        assert!(!adb_reverse_has(text, 7717, 7737));
        assert!(!adb_reverse_has("", 7717, 7717));
    }

    #[test]
    fn owner_display() {
        let o = PortOwner { pid: Some(1), name: Some("nginx".into()), uid: None };
        assert_eq!(o.to_string(), "nginx（pid 1）");
        assert!(PortOwner { pid: None, name: None, uid: Some(0) }.to_string().contains("uid 0"));
    }

    /// 本进程监听一个端口，应能查到自己。
    #[cfg(any(target_os = "linux", windows))]
    #[test]
    fn finds_own_listener() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let owner = listening_owner(port).unwrap().expect("应查到监听者");
        assert_eq!(owner.pid, Some(std::process::id()));
        let exe = std::env::current_exe().unwrap();
        let stem = exe.file_stem().unwrap().to_string_lossy().into_owned();
        let name = owner.name.expect("进程名");
        // Linux comm 最长 15 字节
        assert!(stem.starts_with(&name) || name.starts_with(&stem), "{name} vs {stem}");
        drop(l);
        assert!(process_alive(std::process::id()));
        assert!(process_name(std::process::id()).is_some());
    }
}
