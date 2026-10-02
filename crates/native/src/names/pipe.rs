//! Windows：每 App 每用户命名管道上的名字登记（spec/naming.md 4.3）。
//!
//! 名字 `\\.\pipe\appmcp-<SID>-<appId>`（默认）与 `\\.\pipe\appmcp-<SID>-<appId>.<instance>`（实例）。App 是管道属主：
//! 以 `FILE_FLAG_FIRST_PIPE_INSTANCE` 创建第一个实例（名字已被占用即失败），安全描述符只允许当前用户、拒绝远程客户端
//! （与 Host 的本地 IPC 相同，spec/protocol.md 1.4）。Hub 作为管道客户端打开它，这个管道连接就是通道：
//! 之后与 D-Bus `Open()` 拨入的通道相同，SDK 在其上作为 WebSocket 客户端先发 `app/hello`。
//!
//! 拒绝（已有连接 / SDK 已停止）：Windows 不能像 D-Bus 那样以方法错误回复，App 在管道上写一行 `<CODE>：<说明>\n`
//! 后断开；Hub 读到的第一段数据不是 HTTP 升级请求即按拒绝处理（spec/naming.md 4.3"拒绝"）。

use std::io;
use std::sync::Arc;
use std::time::Duration;

use app_mcp_protocol::endpoint::{check_pipe_name, win};
use app_mcp_protocol::naming::{Address, pipe as names};
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use tokio::task::JoinHandle;

use super::{ChannelSink, NameRequest, NameServer, RegisterFuture, Registration};

/// `ERROR_ACCESS_DENIED`：带 `FILE_FLAG_FIRST_PIPE_INSTANCE` 创建已存在的管道时返回。
const ERROR_ACCESS_DENIED: i32 = 5;
/// 创建下一个管道实例失败时的重试次数与间隔（只在失败路径上计时，空闲时没有定时器）。
const CREATE_RETRIES: u32 = 3;
const CREATE_RETRY_DELAY: Duration = Duration::from_millis(100);

pub(crate) struct PipeNameServer;

impl NameServer for PipeNameServer {
    fn register<'a>(&'a self, request: &'a NameRequest, sink: Arc<dyn ChannelSink>) -> RegisterFuture<'a> {
        Box::pin(async move { register(request, sink).map(|r| Box::new(r) as Box<dyn Registration>) })
    }
}

/// 已登记的管道；被丢弃时停止接受任务，管道实例随之关闭，名字消失。
struct PipeRegistration {
    names: Vec<String>,
    tasks: Vec<JoinHandle<()>>,
}

impl Registration for PipeRegistration {
    fn names(&self) -> Vec<String> {
        self.names.clone()
    }
}

impl Drop for PipeRegistration {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}

/// 登记默认名字（已被同 App 的其他进程拥有时只登记实例名字，与 Linux 相同）与实例名字。必须在运行时内调用。
fn register(request: &NameRequest, sink: Arc<dyn ChannelSink>) -> Result<PipeRegistration, String> {
    let sid = win::current_user_sid().map_err(|e| format!("无法取得当前用户 SID：{e}"))?;
    let security = Arc::new(win::PipeSecurity::current_user().map_err(|e| format!("无法创建管道安全描述符：{e}"))?);
    let default = Address::new(&request.app_id, None).map_err(|e| e.to_string())?;
    let instance = match &request.instance {
        Some(i) => Some(Address::new(&request.app_id, Some(i)).map_err(|e| e.to_string())?),
        None => None,
    };
    let mut reg = PipeRegistration { names: Vec::new(), tasks: Vec::new() };
    let default_name = names::pipe_name(&sid, &default);
    match bind(&default_name, &security)? {
        Some(first) => reg.serve(default_name, &security, first, &sink),
        None if instance.is_none() => return Err(format!("命名管道 {default_name} 已被其他进程占用，未登记")),
        None => {}
    }
    if let Some(inst) = &instance {
        let name = names::pipe_name(&sid, inst);
        // 失败时 reg 被丢弃，已登记的默认名字随之注销。
        let first = bind(&name, &security)?.ok_or_else(|| format!("命名管道 {name} 已被其他进程占用，未登记"))?;
        reg.serve(name, &security, first, &sink);
    }
    Ok(reg)
}

impl PipeRegistration {
    fn serve(&mut self, name: String, security: &Arc<win::PipeSecurity>, first: NamedPipeServer, sink: &Arc<dyn ChannelSink>) {
        self.tasks.push(tokio::spawn(serve(name.clone(), security.clone(), first, sink.clone())));
        self.names.push(name);
    }
}

/// 以 `FILE_FLAG_FIRST_PIPE_INSTANCE` 创建第一个实例；名字已存在时为 `None`。
fn bind(name: &str, security: &win::PipeSecurity) -> Result<Option<NamedPipeServer>, String> {
    // @why 先于创建检查：超长名称给出 IPC_PATH_TOO_LONG 与建议，而不是系统的 ERROR_INVALID_NAME。
    check_pipe_name(name).map_err(|issue| issue.to_string())?;
    match create(name, security, true) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.raw_os_error() == Some(ERROR_ACCESS_DENIED) => Ok(None),
        Err(e) => Err(format!("创建命名管道 {name} 失败：{e}")),
    }
}

/// 创建一个管道实例（只允许当前用户、拒绝远程客户端）。
///
/// @why 与 Hub 的 IPC 监听（`crates/hub/src/ipc.rs`）相同的三行：两处依赖 tokio 的 crate 互不依赖，
/// 而共用的安全描述符已在 `app_mcp_protocol::endpoint::win::PipeSecurity`（protocol 不依赖 tokio）。
fn create(name: &str, security: &win::PipeSecurity, first: bool) -> io::Result<NamedPipeServer> {
    let mut opts = ServerOptions::new();
    opts.first_pipe_instance(first).reject_remote_clients(true);
    security.with_attributes(|sa| {
        // SAFETY: sa 指向有效的 SECURITY_ATTRIBUTES，在调用期间有效。
        unsafe { opts.create_with_security_attributes_raw(name, sa) }
    })
}

/// 创建后续实例（名字已由第一个实例持有）；失败时短暂重试。
async fn create_next(name: &str, security: &win::PipeSecurity) -> io::Result<NamedPipeServer> {
    let mut attempt = 0;
    loop {
        match create(name, security, false) {
            Ok(s) => return Ok(s),
            Err(e) if attempt + 1 >= CREATE_RETRIES => return Err(e),
            Err(_) => {
                attempt += 1;
                tokio::time::sleep(CREATE_RETRY_DELAY).await;
            }
        }
    }
}

/// 接受任务：等待 Hub 打开管道 → 先备好下一个实例（名字不中断）→ 把这条连接交给运行时，或写拒绝行后断开。
/// 空闲时阻塞在 `ConnectNamedPipe` 上（无定时器、不轮询）。
///
/// @error 无法再创建管道实例（系统资源耗尽）时任务结束，名字随最后一个实例关闭而消失；之后 Hub 的拨号会激活新进程。
async fn serve(name: String, security: Arc<win::PipeSecurity>, first: NamedPipeServer, sink: Arc<dyn ChannelSink>) {
    let mut next = Some(first);
    loop {
        let server = match next.take() {
            Some(s) => s,
            None => match create_next(&name, &security).await {
                Ok(s) => s,
                Err(_) => return,
            },
        };
        // 失败时该实例已不可用（如客户端在连接前放弃）：丢弃，下一轮重新创建。
        if server.connect().await.is_err() {
            continue;
        }
        next = create_next(&name, &security).await.ok();
        if let Err((refusal, server)) = sink.try_offer(server) {
            tokio::spawn(super::refuse(server, refusal));
        }
    }
}
