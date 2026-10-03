//! 监听与接受连接：TCP、Unix 域套接字、Windows 命名管道。

use super::*;

impl Listener {
    /// 绑定并返回 `LISTENING` 行要打印的位置（TCP 为 `host:port`，IPC 为端点字符串）。
    pub(super) async fn bind(opts: &Options) -> Result<(Self, String), String> {
        let Some(endpoint) = &opts.ipc else {
            let addr = opts.addr.as_deref().unwrap_or(DEFAULT_ADDR);
            let listener = TcpListener::bind(addr)
                .await
                .map_err(|e| format!("无法绑定 {addr}：{e}"))?;
            let local = listener
                .local_addr()
                .map_err(|e| format!("无法获取监听地址：{e}"))?;
            return Ok((Self::Tcp(listener), local.to_string()));
        };
        let listener = match endpoint {
            #[cfg(unix)]
            Endpoint::Unix(path) => Self::Unix(UnixSocket::bind(path)?),
            #[cfg(windows)]
            Endpoint::Pipe(name) => Self::Pipe(PipeListener::bind(name)?),
            other => return Err(format!("本平台不支持端点 {other}")),
        };
        Ok((listener, endpoint.to_string()))
    }

    pub(super) async fn accept(&mut self) -> Result<Box<dyn Io>, String> {
        let accept_err = |e: std::io::Error| format!("accept 失败：{e}");
        match self {
            Self::Tcp(l) => Ok(Box::new(l.accept().await.map_err(accept_err)?.0)),
            #[cfg(unix)]
            Self::Unix(s) => Ok(Box::new(s.listener.accept().await.map_err(accept_err)?.0)),
            #[cfg(windows)]
            Self::Pipe(p) => Ok(Box::new(p.accept().await.map_err(accept_err)?)),
        }
    }
}

#[cfg(unix)]
impl UnixSocket {
    /// @error 路径过长（IPC_PATH_TOO_LONG）、路径已存在或目录不可写时返回错误；不删除已有文件。
    fn bind(path: &std::path::Path) -> Result<Self, String> {
        proto::endpoint::check_unix_socket_path(path).map_err(|i| i.to_string())?;
        let listener = tokio::net::UnixListener::bind(path)
            .map_err(|e| format!("无法绑定 unix:{}：{e}", path.display()))?;
        Ok(Self { listener, path: path.to_owned() })
    }
}

#[cfg(unix)]
impl Drop for UnixSocket {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(windows)]
impl PipeListener {
    /// @error 名称过长（IPC_PATH_TOO_LONG）或同名管道已存在时返回错误。
    fn bind(name: &str) -> Result<Self, String> {
        proto::endpoint::check_pipe_name(name).map_err(|i| i.to_string())?;
        let security = proto::endpoint::win::PipeSecurity::current_user()
            .map_err(|e| format!("无法创建管道安全描述符：{e}"))?;
        let next = Self::create(name, &security, true)
            .map_err(|e| format!("无法创建命名管道 {name}：{e}"))?;
        Ok(Self { name: name.to_owned(), security, next })
    }

    fn create(
        name: &str,
        security: &proto::endpoint::win::PipeSecurity,
        first: bool,
    ) -> std::io::Result<tokio::net::windows::named_pipe::NamedPipeServer> {
        let mut opts = tokio::net::windows::named_pipe::ServerOptions::new();
        opts.first_pipe_instance(first).reject_remote_clients(true);
        security.with_attributes(|sa| {
            // SAFETY: sa 指向有效的 SECURITY_ATTRIBUTES，在调用期间有效。
            unsafe { opts.create_with_security_attributes_raw(name, sa) }
        })
    }

    async fn accept(&mut self) -> std::io::Result<tokio::net::windows::named_pipe::NamedPipeServer> {
        self.next.connect().await?;
        let next = Self::create(&self.name, &self.security, false)?;
        Ok(std::mem::replace(&mut self.next, next))
    }
}
