//! 本地 IPC 监听（spec/protocol.md 1.2）：Unix 域套接字 / Windows 命名管道。
//!
//! 流上跑的是与 TCP 相同的 WebSocket + JSON-RPC（由 [`crate::app_server`] 处理），这里只负责
//! 绑定、单实例判断、连接鉴权与对端进程号：
//!
//! - Unix：套接字所在目录必须属于当前用户且组 / 其他用户不可写（Hub 新建的目录为 0700），套接字文件 0600；
//!   每个连接用 `SO_PEERCRED` / `getpeereid` 检查对端有效用户 ID 与本进程相同，不同则直接断开。
//!   路径上已有套接字时先尝试连接：能连上 = 另一个 Hub 正在监听（[`io::ErrorKind::AddrInUse`]），
//!   连接被拒绝 = 上次异常退出留下的文件，删除后重新绑定。
//! - Windows：管道的安全描述符只允许当前用户访问、所有者为当前用户（SDDL `O:<sid>D:P(A;;GA;;;<sid>)`），
//!   拒绝远程客户端；第一个实例带 `FILE_FLAG_FIRST_PIPE_INSTANCE`，同名管道已存在时返回
//!   [`io::ErrorKind::AddrInUse`]。客户端进程号由 `GetNamedPipeClientProcessId` 取得。

use std::io;

/// 一个已接受的 IPC 连接。
pub(crate) struct Accepted<S> {
    pub stream: S,
    /// 对端进程号（平台不提供时为 `None`）。
    pub pid: Option<u32>,
}

#[cfg(unix)]
pub(crate) use unix::{IpcListener, prepare_dir as prepare_private_dir};
#[cfg(windows)]
pub(crate) use windows::IpcListener;

fn invalid(msg: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, msg)
}

#[cfg(unix)]
mod unix {
    use std::io;
    use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt};
    use std::path::{Path, PathBuf};

    use app_mcp_protocol::Endpoint;
    use app_mcp_protocol::endpoint::current_uid;
    use tokio::net::{UnixListener, UnixStream};

    use super::{Accepted, invalid};

    pub(crate) struct IpcListener {
        listener: UnixListener,
        /// 绑定时的套接字文件身份 `(dev, ino)`：停止时只删除仍是自己创建的那个文件。
        /// 服务管理器交来的套接字（[`IpcListener::adopt`]）为 `None`：文件归服务管理器，不删除。
        file: Option<(PathBuf, u64, u64)>,
    }

    impl IpcListener {
        pub async fn bind(endpoint: &Endpoint) -> io::Result<Self> {
            let Endpoint::Unix(path) = endpoint else {
                return Err(invalid(format!(
                    "本平台的本地 IPC 端点必须是 unix:<绝对路径>：{endpoint}"
                )));
            };
            if !path.is_absolute() {
                return Err(invalid(format!("套接字路径必须是绝对路径：{}", path.display())));
            }
            // @why 先于建目录检查：超长路径（IPC_PATH_TOO_LONG）不留下任何文件系统改动。
            app_mcp_protocol::endpoint::check_unix_socket_path(path)
                .map_err(|issue| io::Error::new(io::ErrorKind::InvalidInput, issue))?;
            let parent = path
                .parent()
                .ok_or_else(|| invalid(format!("套接字路径没有上级目录：{}", path.display())))?;
            prepare_dir(parent)?;
            remove_stale(path)?;
            let listener = UnixListener::bind(path)?;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
            let meta = std::fs::symlink_metadata(path)?;
            Ok(Self {
                listener,
                file: Some((path.clone(), meta.dev(), meta.ino())),
            })
        }

        /// 接管服务管理器（systemd 套接字激活 / launchd `Sockets`）交来的监听套接字，返回 `(端点字符串, 监听器)`。
        ///
        /// @security 与 [`IpcListener::bind`] 相同：套接字所在目录必须属于当前用户且组 / 其他用户不可写；
        /// 每个连接仍核对对端用户。
        /// @error 套接字没有文件路径（匿名 / 抽象命名空间）、目录不安全。
        pub fn adopt(std: std::os::unix::net::UnixListener) -> io::Result<(String, Self)> {
            let addr = std.local_addr()?;
            let path = addr
                .as_pathname()
                .ok_or_else(|| invalid("交来的本地 IPC 套接字没有文件路径（匿名或抽象命名空间）".to_owned()))?
                .to_path_buf();
            let parent = path
                .parent()
                .ok_or_else(|| invalid(format!("套接字路径没有上级目录：{}", path.display())))?;
            check_dir(parent)?;
            std.set_nonblocking(true)?;
            let listener = UnixListener::from_std(std)?;
            Ok((Endpoint::Unix(path).to_string(), Self { listener, file: None }))
        }

        /// 接受下一个连接。对端用户不同的连接直接关闭并继续等待。
        pub async fn accept(&mut self) -> io::Result<Accepted<UnixStream>> {
            loop {
                let (stream, _) = self.listener.accept().await?;
                let cred = match stream.peer_cred() {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::warn!("无法读取 IPC 对端凭据，断开：{e}");
                        continue;
                    }
                };
                if cred.uid() != current_uid() {
                    tracing::warn!(
                        uid = cred.uid(),
                        pid = ?cred.pid(),
                        "拒绝其他用户的 IPC 连接"
                    );
                    continue;
                }
                let pid = cred.pid().and_then(|p| u32::try_from(p).ok());
                return Ok(Accepted { stream, pid });
            }
        }
    }

    impl Drop for IpcListener {
        fn drop(&mut self) {
            let Some((path, dev, ino)) = &self.file else { return };
            if let Ok(m) = std::fs::symlink_metadata(path)
                && m.dev() == *dev
                && m.ino() == *ino
            {
                let _ = std::fs::remove_file(path);
            }
        }
    }

    /// 目录不存在时以 0700 创建；存在时必须属于当前用户且组 / 其他用户不可写。
    pub(crate) fn prepare_dir(dir: &Path) -> io::Result<()> {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        check_dir(dir)
    }

    /// 目录必须属于当前用户且组 / 其他用户不可写。
    fn check_dir(dir: &Path) -> io::Result<()> {
        let meta = std::fs::metadata(dir)?;
        if meta.uid() != current_uid() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("目录 {} 不属于当前用户", dir.display()),
            ));
        }
        if meta.mode() & 0o022 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "目录 {} 对组或其他用户可写（权限 {:o}），请改为 0700",
                    dir.display(),
                    meta.mode() & 0o777
                ),
            ));
        }
        Ok(())
    }

    /// 路径上已有文件：活着的套接字 → `AddrInUse`；无人监听的套接字 → 删除；其他文件 → 报错。
    fn remove_stale(path: &Path) -> io::Result<()> {
        let meta = match std::fs::symlink_metadata(path) {
            Ok(m) => m,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };
        if !meta.file_type().is_socket() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("{} 已存在且不是套接字，拒绝覆盖", path.display()),
            ));
        }
        match std::os::unix::net::UnixStream::connect(path) {
            Ok(_) => Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                format!("已有 Hub 在监听 {}", path.display()),
            )),
            Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => {
                tracing::info!(path = %path.display(), "删除残留的套接字文件");
                match std::fs::remove_file(path) {
                    Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
                    _ => Ok(()),
                }
            }
            Err(e) => Err(e),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn temp_dir(mode: u32) -> PathBuf {
            let dir = std::env::temp_dir().join(format!("amcp-adopt-{:016x}", rand::random::<u64>()));
            std::fs::create_dir(&dir).unwrap();
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(mode)).unwrap();
            dir
        }

        /// 服务管理器交来的套接字：端点取自其路径；停止时不删除文件（文件归服务管理器）；仍只接受同一用户。
        #[tokio::test]
        async fn adopted_socket_keeps_file_and_accepts() {
            let dir = temp_dir(0o700);
            let path = dir.join("hub.sock");
            let std = std::os::unix::net::UnixListener::bind(&path).unwrap();
            let (endpoint, mut l) = IpcListener::adopt(std).unwrap();
            assert_eq!(endpoint, format!("unix:{}", path.display()));
            let client = tokio::spawn({
                let path = path.clone();
                async move { UnixStream::connect(path).await.unwrap() }
            });
            let accepted = l.accept().await.unwrap();
            assert_eq!(accepted.pid, Some(std::process::id()));
            drop(client.await.unwrap());
            drop(l);
            assert!(path.exists(), "交来的套接字文件不应被 Hub 删除");
            std::fs::remove_dir_all(&dir).unwrap();
        }

        #[tokio::test]
        async fn adopted_socket_in_writable_dir_is_refused() {
            let dir = temp_dir(0o777);
            let std = std::os::unix::net::UnixListener::bind(dir.join("hub.sock")).unwrap();
            let err = IpcListener::adopt(std).err().unwrap();
            assert_eq!(err.kind(), io::ErrorKind::PermissionDenied, "{err}");
            std::fs::remove_dir_all(&dir).unwrap();
        }

        /// 所在目录已不存在（无法核对属主与权限）：拒绝接管。
        #[tokio::test]
        async fn adopted_socket_without_directory_is_refused() {
            let dir = temp_dir(0o700);
            let std = std::os::unix::net::UnixListener::bind(dir.join("x.sock")).unwrap();
            std::fs::remove_dir_all(&dir).unwrap();
            assert_eq!(IpcListener::adopt(std).err().unwrap().kind(), io::ErrorKind::NotFound);
        }
    }
}

#[cfg(windows)]
mod windows {
    use std::io;
    use std::os::windows::io::AsRawHandle;

    use app_mcp_protocol::Endpoint;
    use app_mcp_protocol::endpoint::win::{PipeSecurity, pipe_client_pid};
    use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};

    use super::{Accepted, invalid};

    /// `ERROR_ACCESS_DENIED`：带 `FILE_FLAG_FIRST_PIPE_INSTANCE` 创建已存在的管道时返回。
    const ERROR_ACCESS_DENIED: i32 = 5;

    pub(crate) struct IpcListener {
        name: String,
        security: PipeSecurity,
        /// 等待下一个客户端的管道实例；创建失败时为 `None`，下次 accept 时重试。
        next: Option<NamedPipeServer>,
    }

    impl IpcListener {
        pub async fn bind(endpoint: &Endpoint) -> io::Result<Self> {
            let Endpoint::Pipe(name) = endpoint else {
                return Err(invalid(format!(
                    r"本平台的本地 IPC 端点必须是 pipe:\\.\pipe\<名称>：{endpoint}"
                )));
            };
            // @why 先于创建管道检查：超长名称给出 IPC_PATH_TOO_LONG 与建议，而不是系统的 ERROR_INVALID_NAME。
            app_mcp_protocol::endpoint::check_pipe_name(name)
                .map_err(|issue| io::Error::new(io::ErrorKind::InvalidInput, issue))?;
            let security = PipeSecurity::current_user()?;
            let first = create(name, &security, true).map_err(|e| {
                if e.raw_os_error() == Some(ERROR_ACCESS_DENIED) {
                    io::Error::new(
                        io::ErrorKind::AddrInUse,
                        format!("命名管道 {name} 已存在（另一个 Hub 正在运行，或被其他程序占用）"),
                    )
                } else {
                    e
                }
            })?;
            Ok(Self {
                name: name.clone(),
                security,
                next: Some(first),
            })
        }

        pub async fn accept(&mut self) -> io::Result<Accepted<NamedPipeServer>> {
            let server = match self.next.take() {
                Some(s) => s,
                None => create(&self.name, &self.security, false)?,
            };
            // 失败时该实例已不可用（如客户端在连接前放弃）：丢弃，下次重新创建。
            server.connect().await?;
            match create(&self.name, &self.security, false) {
                Ok(s) => self.next = Some(s),
                Err(e) => tracing::warn!("创建下一个管道实例失败：{e}"),
            }
            let pid = pipe_client_pid(server.as_raw_handle());
            Ok(Accepted {
                stream: server,
                pid,
            })
        }
    }

    fn create(name: &str, security: &PipeSecurity, first: bool) -> io::Result<NamedPipeServer> {
        let mut opts = ServerOptions::new();
        opts.first_pipe_instance(first).reject_remote_clients(true);
        security.with_attributes(|sa| {
            // SAFETY: sa 指向有效的 SECURITY_ATTRIBUTES，在调用期间有效。
            unsafe { opts.create_with_security_attributes_raw(name, sa) }
        })
    }
}
