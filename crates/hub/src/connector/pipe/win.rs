//! [`super::PipeSystem`] 的 Windows 实现：打开管道并核对身份、按登记的方式激活、监视登记目录。
//!
//! @security 本文件集中了连接器的全部 Win32 调用（G-06）；句柄都由 [`OwnedHandle`] 持有并在 Drop 时关闭。

use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;

use app_mcp_protocol::endpoint::win as ident;
use app_mcp_protocol::naming::registration::{Registration, kinds};
use app_mcp_protocol::naming::{codes, pipe as names};
use app_mcp_protocol::{WakeDescriptor, WakeKind};
use tokio::net::windows::named_pipe::ClientOptions;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OVERLAPPED, FILE_LIST_DIRECTORY, FILE_NOTIFY_CHANGE_FILE_NAME,
    FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    ReadDirectoryChangesW,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Pipes::GetNamedPipeServerProcessId;
use windows_sys::Win32::System::Threading::{
    CreateEventW, INFINITE, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    QueryFullProcessImageNameW, SetEvent, WaitForMultipleObjects,
};

use super::{DirChange, DirWatch, Launched, OpenedPipe, PipeOpen, PipeSystem, parse_notify};
use crate::connector::ConnectorError;
use crate::wake::{SystemWaker, WakeRequest, Waker};

const ERROR_FILE_NOT_FOUND: i32 = 2;
const ERROR_ACCESS_DENIED: i32 = 5;
const ERROR_PIPE_BUSY: i32 = 231;
/// 目录通知缓冲（`ReadDirectoryChangesW`，DWORD 对齐）：16 KiB。
const NOTIFY_BUFFER_WORDS: usize = 4096;
/// `QueryFullProcessImageNameW` 的缓冲（长路径上限）。
const IMAGE_PATH_CHARS: usize = 32_768;

pub(super) struct WinPipes;

/// 拥有的内核句柄；Drop 时关闭。
struct OwnedHandle(HANDLE);

// SAFETY: 内核句柄可以在线程间传递与共享；只在 Drop 时关闭一次。
unsafe impl Send for OwnedHandle {}
// SAFETY: 同上（SetEvent / WaitForMultipleObjects 对同一事件句柄是线程安全的）。
unsafe impl Sync for OwnedHandle {}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: 句柄由本结构独占，只关闭一次。
        unsafe { CloseHandle(self.0) };
    }
}

fn manual_reset_event() -> std::io::Result<OwnedHandle> {
    // SAFETY: 参数均为空指针 / 常量；返回空句柄表示失败。
    let h = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
    if h.is_null() { Err(std::io::Error::last_os_error()) } else { Ok(OwnedHandle(h)) }
}

/// 管道服务端进程号。
fn server_pid(pipe: HANDLE) -> Option<u32> {
    let mut pid = 0u32;
    // SAFETY: pipe 是有效的管道客户端句柄。
    (unsafe { GetNamedPipeServerProcessId(pipe, &mut pid) } != 0).then_some(pid)
}

/// 进程映像的完整路径（Win32 形式，`C:\…`）。
fn process_image(pid: u32) -> Option<String> {
    // SAFETY: 只请求查询受限信息；失败返回空句柄。
    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if h.is_null() {
        return None;
    }
    let process = OwnedHandle(h);
    let mut buf = vec![0u16; IMAGE_PATH_CHARS];
    let mut len = u32::try_from(buf.len()).unwrap_or(u32::MAX);
    // SAFETY: buf 有 len 个 u16；成功时 len 为写入的字符数（不含结尾 0）。
    let ok = unsafe { QueryFullProcessImageNameW(process.0, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) };
    (ok != 0).then(|| String::from_utf16_lossy(buf.get(..len as usize).unwrap_or_default()))
}

/// `exec` 激活拉起的进程。
struct ChildLaunch(Child);

impl Launched for ChildLaunch {
    fn failure(&mut self) -> Option<String> {
        match self.0.try_wait() {
            Ok(Some(status)) if !status.success() => Some(format!("进程退出（{status}）")),
            _ => None,
        }
    }
}

/// `uri` / `aumid` 激活：系统转交，看不到进程。
struct Handed;

impl Launched for Handed {
    fn failure(&mut self) -> Option<String> {
        None
    }
}

/// 停止目录通知线程（被丢弃时置位停止事件）。
struct StopOnDrop(Arc<OwnedHandle>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        // SAFETY: 事件句柄由 Arc 保持有效。
        unsafe { SetEvent(self.0.0) };
    }
}

fn exec_program(reg: &Registration) -> &str {
    let target = reg.activation.target.as_str();
    if target.is_empty() { reg.executable.as_deref().unwrap_or_default() } else { target }
}

#[async_trait::async_trait]
impl PipeSystem for WinPipes {
    fn open(&self, name: &str) -> Result<PipeOpen, ConnectorError> {
        let client = match ClientOptions::new().open(name) {
            Ok(c) => c,
            Err(e) => {
                return match e.raw_os_error() {
                    Some(ERROR_FILE_NOT_FOUND) => Ok(PipeOpen::Absent),
                    Some(ERROR_PIPE_BUSY) => Ok(PipeOpen::Busy),
                    Some(ERROR_ACCESS_DENIED) => {
                        Err(ConnectorError::new(codes::BIND_PERMISSION_DENIED, format!("无权打开管道 {name}：{e}")))
                    }
                    _ => Err(ConnectorError::new(codes::ACTIVATION_DENIED, format!("打开管道 {name} 失败：{e}"))),
                };
            }
        };
        let handle = client.as_raw_handle() as HANDLE;
        // @security 管道所有者必须是当前用户（spec/naming.md 4.3、10.1）：他人无法以我们的 SID 为所有者创建对象。
        let mismatch = |why: String| ConnectorError::new(codes::PEER_IDENTITY_MISMATCH, why);
        let owner = ident::handle_owner_sid(client.as_raw_handle()).map_err(|e| mismatch(format!("无法读取管道 {name} 的所有者：{e}")))?;
        let me = ident::current_user_sid().map_err(|e| mismatch(format!("无法取得当前用户 SID：{e}")))?;
        if owner != me {
            return Err(mismatch(format!("管道 {name} 的所有者（{owner}）不是当前用户，已拒绝")));
        }
        let pid = server_pid(handle);
        let server_image = pid.and_then(process_image);
        Ok(PipeOpen::Opened(OpenedPipe { stream: Box::new(client), server_pid: pid, server_image }))
    }

    async fn activate(&self, reg: &Registration) -> Result<Box<dyn Launched>, ConnectorError> {
        let denied = |why: String| ConnectorError::new(codes::ACTIVATION_DENIED, why);
        let (kind, arg) = match reg.activation.kind.as_str() {
            kinds::EXEC => {
                let program = exec_program(reg);
                let mut cmd = Command::new(program);
                cmd.arg(names::ACTIVATION_ARG)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .creation_flags(crate::CREATE_NO_WINDOW);
                if let Some(dir) = Path::new(program).parent().filter(|d| d.is_dir()) {
                    cmd.current_dir(dir);
                }
                tracing::info!(app_id = %reg.app_id, program, "exec 激活");
                return match cmd.spawn() {
                    Ok(child) => Ok(Box::new(ChildLaunch(child))),
                    // 程序不存在：与 D-Bus `Spawn.ExecFailed` 相同按"未安装"处理（spec/naming.md 4.1、5.4）。
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        Err(ConnectorError::new(codes::NAME_NOT_FOUND, format!("激活程序 {program} 不存在：{e}")))
                    }
                    Err(e) => Err(denied(format!("无法运行激活程序 {program}：{e}"))),
                };
            }
            kinds::URI => (WakeKind::Uri, None),
            kinds::AUMID => (WakeKind::Aumid, Some(names::ACTIVATION_ARG.to_owned())),
            other => return Err(denied(format!("不支持的激活方式「{other}」"))),
        };
        // 复用唤醒器的协议 / AUMID 激活（spec/lifecycle.md 第 5 节的同一实现）；令牌只为满足 URI 形式，Hub 不据此认领。
        let token = crate::wake::new_token();
        let req = WakeRequest {
            app_id: reg.app_id.clone(),
            instance_id: None,
            descriptor: WakeDescriptor { kind, target: Some(reg.activation.target.clone()), background: true },
            activation_arg: arg.unwrap_or_else(|| format!("app-mcp-wake:{token}")),
            token,
        };
        SystemWaker::new().wake(req).await.map_err(|e| denied(e.to_string()))?;
        Ok(Box::new(Handed))
    }

    fn watch(&self, dir: &Path) -> Result<Option<DirWatch>, ConnectorError> {
        let fail = |what: &str, e: std::io::Error| {
            ConnectorError::new(codes::NAME_NOT_FOUND, format!("无法监视登记目录 {}（{what}）：{e}", dir.display()))
        };
        std::fs::create_dir_all(dir).map_err(|e| fail("创建目录", e))?;
        let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
        // SAFETY: wide 以 0 结尾；其余参数为常量 / 空指针。
        let h = unsafe {
            CreateFileW(
                wide.as_ptr(),
                FILE_LIST_DIRECTORY,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
                std::ptr::null_mut(),
            )
        };
        if h == INVALID_HANDLE_VALUE {
            return Err(fail("打开目录", std::io::Error::last_os_error()));
        }
        let dir_handle = OwnedHandle(h);
        let io_event = manual_reset_event().map_err(|e| fail("创建事件", e))?;
        let stop = Arc::new(manual_reset_event().map_err(|e| fail("创建事件", e))?);
        let (tx, rx) = unbounded_channel();
        let thread_stop = stop.clone();
        let shown = dir.display().to_string();
        // @why 一个阻塞在 WaitForMultipleObjects 上的线程（无定时器、不轮询）：spec/naming.md 5.1 的目录变更通知；
        // 只在启用按名寻址的 Hub 中存在，连接器的事件流被丢弃即退出。
        std::thread::Builder::new()
            .name("app-mcp-pipe-watch".into())
            .spawn(move || watch_loop(&dir_handle, &io_event, &thread_stop, &tx, &shown))
            .map_err(|e| fail("创建线程", e))?;
        Ok(Some(DirWatch { rx, guard: Box::new(StopOnDrop(stop)) }))
    }
}

/// 目录通知循环：每次完成的 `ReadDirectoryChangesW` 产生一个 [`DirChange`]；停止事件或接收端关闭时结束。
fn watch_loop(dir: &OwnedHandle, io_event: &OwnedHandle, stop: &OwnedHandle, tx: &UnboundedSender<DirChange>, shown: &str) {
    let mut buf = vec![0u32; NOTIFY_BUFFER_WORDS];
    let byte_len = u32::try_from(buf.len() * 4).unwrap_or(u32::MAX);
    loop {
        // SAFETY: OVERLAPPED 是纯数据结构，全零为合法初值。
        let mut ov: OVERLAPPED = unsafe { std::mem::zeroed() };
        ov.hEvent = io_event.0;
        // SAFETY: buf 至少 byte_len 字节且 DWORD 对齐；ov 在本轮等待结束前一直有效（下方等待完成或取消后才离开作用域）。
        let started = unsafe {
            ReadDirectoryChangesW(
                dir.0,
                buf.as_mut_ptr().cast::<c_void>(),
                byte_len,
                0,
                FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_LAST_WRITE,
                std::ptr::null_mut(),
                &mut ov,
                None,
            )
        };
        if started == 0 {
            tracing::warn!(dir = shown, error = %std::io::Error::last_os_error(), "登记目录通知失败，停止监视");
            return;
        }
        let handles = [io_event.0, stop.0];
        // SAFETY: 两个句柄都有效。
        let woke = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE) };
        let mut n = 0u32;
        if woke != WAIT_OBJECT_0 {
            // 停止（或等待失败）：取消进行中的读取并等它结束，之后 ov 与 buf 才能释放。
            // SAFETY: dir 与 ov 有效；GetOverlappedResult(bWait = TRUE) 等待取消完成。
            unsafe {
                CancelIoEx(dir.0, &ov);
                GetOverlappedResult(dir.0, &ov, &mut n, 1);
            }
            return;
        }
        // SAFETY: 读取已完成（事件已置位）。
        if unsafe { GetOverlappedResult(dir.0, &ov, &mut n, 0) } == 0 {
            tracing::warn!(dir = shown, error = %std::io::Error::last_os_error(), "读取登记目录通知失败，停止监视");
            return;
        }
        let change = if n == 0 {
            DirChange::Rescan
        } else {
            // SAFETY: buf 是 u32 数组，按字节查看不越界（长度 = buf.len() * 4）。
            let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), buf.len() * 4) };
            DirChange::Files(parse_notify(bytes.get(..n as usize).unwrap_or(bytes)))
        };
        if tx.send(change).is_err() {
            return;
        }
    }
}
