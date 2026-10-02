//! 登记目录的 kqueue 监视（macOS，[`super::DirWatcher`] 的实现；kqueue 的唯一调用处，G-06）。
//!
//! 一个线程阻塞在 `kevent` 上（无超时、不轮询）：目录的 `EVFILT_VNODE`（项增删改名 `NOTE_WRITE`、目录被删 / 改名）→
//! [`DirChange::Rescan`]（kqueue 不给出文件名）；守卫被丢弃时以 `EVFILT_USER` + `NOTE_TRIGGER` 唤醒线程退出。
//!
//! @compat 原地改写已有文件（不经改名）不改变目录项，不产生通知；`app-mcp-host app install` 以"临时文件 + 改名"写入，会产生通知。

use std::ffi::CString;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::sync::Arc;

use app_mcp_protocol::naming::codes;
use tokio::sync::mpsc::unbounded_channel;

use super::super::ConnectorError;
use super::super::registered::{DirChange, DirWatch};

/// 停止事件的标识（`EVFILT_USER`）。
const STOP_IDENT: usize = 1;

pub(super) struct Kqueue;

impl super::DirWatcher for Kqueue {
    fn watch(&self, dir: &Path) -> Result<Option<DirWatch>, ConnectorError> {
        let fail = |what: &str, e: std::io::Error| {
            ConnectorError::new(codes::NAME_NOT_FOUND, format!("无法监视登记目录 {}（{what}）：{e}", dir.display()))
        };
        // 目录不存在时创建（以便监视；与 Windows 连接器相同）。
        std::fs::create_dir_all(dir).map_err(|e| fail("创建", e))?;
        let path = CString::new(dir.as_os_str().as_bytes()).map_err(|_| fail("路径含 NUL", std::io::ErrorKind::InvalidInput.into()))?;
        // SAFETY: path 是有效的 C 字符串；返回值检查后才包装为 OwnedFd。
        let dir_fd = unsafe { libc::open(path.as_ptr(), libc::O_EVTONLY | libc::O_CLOEXEC) };
        if dir_fd < 0 {
            return Err(fail("打开", std::io::Error::last_os_error()));
        }
        // SAFETY: dir_fd 是刚打开的有效 fd，所有权交给 OwnedFd。
        let dir_fd = unsafe { OwnedFd::from_raw_fd(dir_fd) };
        // SAFETY: kqueue 没有前置条件。
        let kq = unsafe { libc::kqueue() };
        if kq < 0 {
            return Err(fail("kqueue", std::io::Error::last_os_error()));
        }
        // SAFETY: kq 是刚创建的有效 fd。
        let kq = Arc::new(unsafe { OwnedFd::from_raw_fd(kq) });
        let changes = [
            event(
                dir_fd.as_raw_fd() as usize,
                libc::EVFILT_VNODE,
                libc::EV_ADD | libc::EV_CLEAR,
                libc::NOTE_WRITE | libc::NOTE_DELETE | libc::NOTE_RENAME | libc::NOTE_REVOKE,
            ),
            event(STOP_IDENT, libc::EVFILT_USER, libc::EV_ADD | libc::EV_CLEAR, 0),
        ];
        submit(&kq, &changes).map_err(|e| fail("注册事件", e))?;

        let (tx, rx) = unbounded_channel();
        let thread_kq = kq.clone();
        std::thread::Builder::new()
            .name("app-mcp-launchd-watch".to_owned())
            .spawn(move || {
                // dir_fd 随线程存活，线程退出时关闭。
                let _dir = dir_fd;
                while let Some(stop) = wait(&thread_kq) {
                    if stop || tx.send(DirChange::Rescan).is_err() {
                        break;
                    }
                }
            })
            .map_err(|e| fail("启动监视线程", e))?;
        Ok(Some(DirWatch { rx, guard: Box::new(StopOnDrop(kq)) }))
    }
}

/// 守卫：被丢弃时触发停止事件，监视线程随之退出。
struct StopOnDrop(Arc<OwnedFd>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        let _ = submit(&self.0, &[event(STOP_IDENT, libc::EVFILT_USER, 0, libc::NOTE_TRIGGER)]);
    }
}

fn event(ident: usize, filter: i16, flags: u16, fflags: u32) -> libc::kevent {
    libc::kevent { ident, filter, flags, fflags, data: 0, udata: std::ptr::null_mut() }
}

/// 提交事件变更（不取事件）。
fn submit(kq: &OwnedFd, changes: &[libc::kevent]) -> std::io::Result<()> {
    let n = i32::try_from(changes.len()).map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    // SAFETY: changes 指向 n 个有效的 kevent；不取事件（eventlist 为空，nevents 为 0），timeout 为空。
    let rc = unsafe { libc::kevent(kq.as_raw_fd(), changes.as_ptr(), n, std::ptr::null_mut(), 0, std::ptr::null()) };
    if rc < 0 { Err(std::io::Error::last_os_error()) } else { Ok(()) }
}

/// 阻塞等待一个事件：`Some(true)` = 停止，`Some(false)` = 目录变化，`None` = kqueue 出错（线程退出）。
fn wait(kq: &OwnedFd) -> Option<bool> {
    loop {
        let mut out = event(0, 0, 0, 0);
        // SAFETY: out 是一个可写的 kevent，nevents 为 1；无超时（阻塞）。
        let rc = unsafe { libc::kevent(kq.as_raw_fd(), std::ptr::null(), 0, &mut out, 1, std::ptr::null()) };
        if rc < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return None;
        }
        if rc == 0 {
            continue;
        }
        return Some(out.filter == libc::EVFILT_USER);
    }
}
