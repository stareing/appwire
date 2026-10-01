//! 单实例锁与登记文件（spec/protocol.md 1.5、1.7）。
//!
//! - 锁：`<run_dir>/hub.lock`，在任何监听之前以独占、非阻塞方式加锁（Unix `flock`，Windows `LockFileEx`）。
//!   锁随进程退出由操作系统释放，不会因异常退出而残留；已被锁定时 [`Instance::acquire`] 返回
//!   [`io::ErrorKind::ResourceBusy`]（消息中带持有者的 pid 与地址，取自登记文件）。
//! - 登记文件：`<run_dir>/endpoints.json`，绑定完成后原子写入（先写临时文件再改名，Unix 权限 0600），
//!   [`Instance`] 被丢弃（Hub 停止）时删除。
//! - Unix 上 `run_dir` 不存在时以 0700 创建；已存在时必须属于当前用户且组 / 其他用户不可写（与 IPC 套接字目录相同）。

use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use app_mcp_protocol::registry::{EndpointRegistry, LOCK_FILE, REGISTRY_FILE};

/// 持有中的单实例锁；丢弃时删除登记文件并释放锁。
#[derive(Debug)]
pub(crate) struct Instance {
    /// 锁文件句柄：关闭即释放锁。
    _lock: File,
    registry: PathBuf,
    published: bool,
}

impl Instance {
    /// 在 `run_dir` 下取得单实例锁。
    pub fn acquire(run_dir: &Path) -> io::Result<Self> {
        prepare_dir(run_dir)?;
        let path = run_dir.join(LOCK_FILE);
        let mut file = open_lock_file(&path)?;
        let registry = run_dir.join(REGISTRY_FILE);
        if !try_lock(&file)? {
            let holder = match EndpointRegistry::read(&registry) {
                Ok(Some(r)) => format!(
                    "pid {}，版本 {}，监听 {}",
                    r.identity.pid,
                    r.identity.version,
                    r.listen.as_deref().unwrap_or("-")
                ),
                _ => "尚未写入登记文件，可能正在启动".to_owned(),
            };
            return Err(io::Error::new(
                io::ErrorKind::ResourceBusy,
                format!("另一个 app-mcp Host 正在运行（{holder}）：{} 已被锁定", path.display()),
            ));
        }
        // 锁文件内容只供人工排查：持有者的进程号。
        file.set_len(0)?;
        writeln!(file, "{}", std::process::id())?;
        Ok(Self { _lock: file, registry, published: false })
    }

    /// 原子写入登记文件。
    pub fn publish(&mut self, reg: &EndpointRegistry) -> io::Result<()> {
        let text = serde_json::to_vec_pretty(reg).map_err(io::Error::other)?;
        let tmp = self.registry.with_extension(format!("json.{}.tmp", std::process::id()));
        let result = write_private(&tmp, &text).and_then(|()| std::fs::rename(&tmp, &self.registry));
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result?;
        self.published = true;
        Ok(())
    }

    /// 登记文件路径。
    pub fn registry_path(&self) -> &Path {
        &self.registry
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        if self.published {
            let _ = std::fs::remove_file(&self.registry);
        }
    }
}

/// 写入只有当前用户可读写的文件（Unix 0600；Windows 继承用户目录的 ACL）。
fn write_private(path: &Path, data: &[u8]) -> io::Result<()> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    f.write_all(data)?;
    f.sync_all()
}

fn open_lock_file(path: &Path) -> io::Result<File> {
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(path)
}

/// 独占、非阻塞加锁：成功 `true`，已被其他句柄锁定 `false`。
#[cfg(unix)]
fn try_lock(file: &File) -> io::Result<bool> {
    use std::os::fd::AsRawFd;
    // SAFETY: fd 在 file 存活期间有效。
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(true);
    }
    let e = io::Error::last_os_error();
    if e.kind() == io::ErrorKind::WouldBlock {
        Ok(false)
    } else {
        Err(e)
    }
}

#[cfg(windows)]
fn try_lock(file: &File) -> io::Result<bool> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::ERROR_LOCK_VIOLATION;
    use windows_sys::Win32::Storage::FileSystem::{
        LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY, LockFileEx,
    };
    use windows_sys::Win32::System::IO::OVERLAPPED;
    // SAFETY: OVERLAPPED 是普通数据结构，全零有效（从偏移 0 开始锁定）。
    let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
    // SAFETY: 句柄在 file 存活期间有效；overlapped 在调用期间有效（同步句柄，调用返回即完成）。
    let ok = unsafe {
        LockFileEx(
            file.as_raw_handle(),
            LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
            0,
            1,
            0,
            &mut overlapped,
        )
    };
    if ok != 0 {
        return Ok(true);
    }
    let e = io::Error::last_os_error();
    if e.raw_os_error() == Some(ERROR_LOCK_VIOLATION as i32) {
        Ok(false)
    } else {
        Err(e)
    }
}

#[cfg(unix)]
fn prepare_dir(dir: &Path) -> io::Result<()> {
    crate::ipc::prepare_private_dir(dir)
}

#[cfg(windows)]
fn prepare_dir(dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use app_mcp_protocol::identity::HostIdentity;

    fn temp_dir(tag: &str) -> PathBuf {
        let n: u64 = rand::random();
        std::env::temp_dir().join(format!("app-mcp-inst-{tag}-{}-{n:x}", std::process::id()))
    }

    #[test]
    fn lock_is_exclusive_and_registry_is_removed() {
        let dir = temp_dir("lock");
        let mut first = Instance::acquire(&dir).unwrap();
        let reg = EndpointRegistry {
            identity: HostIdentity::current("9.9.9"),
            listen: Some("127.0.0.1:1".into()),
            ipc_endpoint: None,
            started_at_ms: 1,
        };
        first.publish(&reg).unwrap();
        assert_eq!(EndpointRegistry::read(first.registry_path()).unwrap(), Some(reg));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(first.registry_path()).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        // 第二次加锁（同一进程的另一个句柄同样互斥）：ResourceBusy，消息带持有者信息
        let err = Instance::acquire(&dir).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::ResourceBusy);
        assert!(err.to_string().contains("9.9.9"), "{err}");

        let path = first.registry_path().to_owned();
        drop(first);
        assert!(!path.exists(), "停止时删除登记文件");
        // 锁已释放。并行测试中其他线程 fork 子进程时，子进程在 exec 之前短暂持有句柄副本（O_CLOEXEC
        // 在 exec 时才关闭），因此稍等重试。
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            match Instance::acquire(&dir) {
                Ok(i) => break drop(i),
                Err(e) if e.kind() == io::ErrorKind::ResourceBusy && std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                Err(e) => panic!("{e}"),
            }
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn rejects_shared_run_dir() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("perm");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o777)).unwrap();
        let err = Instance::acquire(&dir).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
