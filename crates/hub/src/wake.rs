//! 唤醒（spec/lifecycle.md §5、§9）：[`Waker`] 接口与按平台的默认实现 [`SystemWaker`]。
//!
//! Hub 路由到休眠实例（或未运行但清单声明了 `wake` / `launch` 的 App）时，生成一次性唤醒令牌，
//! 调用当前 [`Waker`] 按 [`WakeDescriptor`] 激活 App，再等待 App 带令牌回连（或同一实例 ID 回连）。
//! 厂商可用 [`crate::Hub::set_waker`] 替换（如 Android 上发送显式广播）。
//!
//! 默认实现只执行固定的程序并逐个传参，**不经 shell 拼接**；令牌、scheme、AUMID、D-Bus 名称、URL
//! 都先校验字符集。子进程的 stdin / stdout / stderr 全部重定向到空设备（Host 的 stdout 专用于 MCP）。

use std::time::Duration;

use app_mcp_manifest::{KnownLaunch, LaunchEntry, Manifest};
use app_mcp_protocol::ErrorKind;
pub use app_mcp_protocol::{WakeDescriptor, WakeKind};
use serde::{Deserialize, Serialize};

use crate::types::HubError;

/// 一次唤醒请求。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WakeRequest {
    pub app_id: String,
    /// 被唤醒的休眠实例；`None` 表示 App 未运行，按清单 `wake` / `launch` 冷启动。
    pub instance_id: Option<String>,
    /// 唤醒描述：休眠时上报的优先，其次清单 `wake.<平台>`，再次由清单 `launch.<平台>` 推导。
    pub descriptor: WakeDescriptor,
    /// 一次性唤醒令牌（32 个十六进制字符，60 秒内有效）。
    pub token: String,
    /// 通用激活参数 `app-mcp-wake:<token>`（SDK 的 `handleWake` 可识别）。
    pub activation_arg: String,
}

/// 按唤醒描述激活 App。
///
/// 返回 `Ok` 表示已发出激活；之后 Hub 等待 App 回连（`HubConfig::wake_timeout`）。
/// 返回的错误作为调用结果交给模型（建议类别 `LAUNCH_FAILED` / `APP_NOT_INSTALLED`）。
#[async_trait::async_trait]
pub trait Waker: Send + Sync {
    async fn wake(&self, req: WakeRequest) -> Result<(), HubError>;
}

/// 唤醒器配置（`HubConfig::waker`，Host 配置 `lifecycle.waker`；spec/hub-api.md 3.5）。
///
/// JSON 形式：`"system"` / `"none"` / `{"exec": [program, ...args]}`。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WakerConfig {
    /// 按平台执行系统激活命令（[`SystemWaker`]）。
    #[default]
    System,
    /// 不唤醒：休眠实例 / 未运行 App 的调用直接返回 `APP_DISCONNECTED`（带清单的 `launchUrl`），
    /// 不生成令牌、不发 `AppWaking`、不询问审批。
    None,
    /// 执行指定程序（[`ExecWaker`]）：参数逐个传递、不经 shell，[`WakeRequest`] 以一行 JSON 写入其 stdin。
    Exec(Vec<String>),
}

impl WakerConfig {
    /// 按配置构造唤醒器；`None` 表示不唤醒。`exec` 为空数组时返回错误。
    pub fn build(&self) -> Result<Option<std::sync::Arc<dyn Waker>>, HubError> {
        Ok(match self {
            WakerConfig::System => Some(std::sync::Arc::new(SystemWaker::new())),
            WakerConfig::None => None,
            WakerConfig::Exec(argv) => Some(std::sync::Arc::new(ExecWaker::new(argv.clone())?)),
        })
    }
}

/// 执行外部程序完成唤醒（厂商脚本、测试替身、平台上没有内置实现的激活方式）。
///
/// - `argv[0]` 为程序，其余为参数，逐个传递、**不经 shell**；
/// - [`WakeRequest`]（camelCase JSON，一行，末尾换行）写入子进程 stdin 后关闭 stdin；
/// - stdout 丢弃（Host 的 stdout 专用于 MCP）；stderr 收集，失败时附在错误信息中；
/// - 退出码 0 表示已发出激活（之后 Hub 等待 App 回连）；非 0 返回 `LAUNCH_FAILED`；
///   超过 `command_timeout` 仍未退出视为已发出（不杀进程）。
#[derive(Clone, Debug)]
pub struct ExecWaker {
    program: String,
    args: Vec<String>,
    pub command_timeout: Duration,
}

impl ExecWaker {
    pub fn new(argv: Vec<String>) -> Result<Self, HubError> {
        let mut it = argv.into_iter();
        let program = it
            .next()
            .filter(|p| !p.is_empty())
            .ok_or_else(|| launch_failed("waker.exec 至少需要一个元素（要执行的程序）"))?;
        Ok(Self {
            program,
            args: it.collect(),
            command_timeout: Duration::from_secs(10),
        })
    }
}

#[async_trait::async_trait]
impl Waker for ExecWaker {
    async fn wake(&self, req: WakeRequest) -> Result<(), HubError> {
        use std::process::Stdio;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut line = serde_json::to_vec(&req).map_err(|e| launch_failed(e.to_string()))?;
        line.push(b'\n');
        tracing::info!(program = %self.program, args = ?self.args, app_id = %req.app_id, "执行唤醒程序");
        let mut command = tokio::process::Command::new(&self.program);
        command
            .args(&self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        command.creation_flags(crate::CREATE_NO_WINDOW);
        let mut child = command
            .spawn()
            .map_err(|e| launch_failed(format!("无法执行唤醒程序 {}：{e}", self.program)))?;
        if let Some(mut stdin) = child.stdin.take() {
            // 程序不读 stdin 时写入可能失败（管道已关闭），不影响结果判定。
            let _ = stdin.write_all(&line).await;
        }
        let mut stderr = child.stderr.take();
        let wait = async {
            let mut err = Vec::new();
            if let Some(s) = stderr.as_mut() {
                let _ = s.take(4096).read_to_end(&mut err).await;
            }
            (child.wait().await, err)
        };
        match tokio::time::timeout(self.command_timeout, wait).await {
            Ok((Ok(status), _)) if status.success() => Ok(()),
            Ok((Ok(status), err)) => {
                let err = String::from_utf8_lossy(&err);
                let err = err.trim();
                Err(launch_failed(if err.is_empty() {
                    format!("唤醒程序 {} 退出码 {status}", self.program)
                } else {
                    format!("唤醒程序 {} 退出码 {status}：{err}", self.program)
                }))
            }
            Ok((Err(e), _)) => Err(launch_failed(format!("等待唤醒程序 {} 失败：{e}", self.program))),
            Err(_) => Ok(()),
        }
    }
}

/// Hub 所在的平台（决定默认 [`SystemWaker`] 的动作与读取清单的哪个平台键）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Windows,
    MacOs,
    Linux,
    Android,
    Ios,
    Other,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(target_os = "windows") {
            Platform::Windows
        } else if cfg!(target_os = "macos") {
            Platform::MacOs
        } else if cfg!(target_os = "android") {
            Platform::Android
        } else if cfg!(target_os = "ios") {
            Platform::Ios
        } else if cfg!(target_os = "linux") {
            Platform::Linux
        } else {
            Platform::Other
        }
    }

    /// 清单中的平台键。
    pub fn manifest_key(self) -> Option<&'static str> {
        match self {
            Platform::Windows => Some("windows"),
            Platform::MacOs => Some("macos"),
            Platform::Linux => Some("linux"),
            Platform::Android => Some("android"),
            Platform::Ios => Some("ios"),
            Platform::Other => None,
        }
    }
}

/// 要执行的命令：程序与逐个传递的参数（不经 shell）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WakeCommand {
    pub program: String,
    pub args: Vec<String>,
}

/// [`SystemWaker`] 对一次唤醒采取的动作（[`SystemWaker::action`] 只生成、不执行）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WakeAction {
    /// 执行系统命令。
    Command(WakeCommand),
    /// Windows 打包应用：`IApplicationActivationManager::ActivateApplication(aumid, arguments, AO_NONE)`。
    /// `arguments` 为 `app-mcp-wake:<token>`，送达 App 的激活参数（UWP `LaunchActivatedEventArgs.Arguments`、
    /// 打包桌面应用的命令行）；App 已在运行时由系统交给现有实例。
    ActivateApplication { aumid: String, arguments: String },
}

/// 默认唤醒实现：按平台执行系统命令。
///
/// | kind | Windows | macOS | Linux |
/// |---|---|---|---|
/// | `uri` | `cmd /c start "" <scheme>://app-mcp/wake?token=<t>` | `open -g <uri>` | `xdg-open <uri>` |
/// | `aumid` | `IApplicationActivationManager::ActivateApplication(aumid, "app-mcp-wake:<t>", AO_NONE)` | — | — |
/// | `apple-event` | — | `open -g -b <bundle> --args app-mcp-wake:<t>` | — |
/// | `dbus` | — | — | `gdbus call --session … org.freedesktop.Application.ActivateAction app-mcp-wake [<'t'>] {}` |
/// | `web-url` | `rundll32 url.dll,FileProtocolHandler <url>#app-mcp-wake=<t>` | `open <url>` | `xdg-open <url>` |
/// | `android-intent` / `none` | 不支持（`LAUNCH_FAILED`） | | |
#[derive(Clone, Debug)]
pub struct SystemWaker {
    platform: Platform,
    /// 等待命令结束的上限；超时视为已发出（不杀进程）。
    pub command_timeout: Duration,
}

impl Default for SystemWaker {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemWaker {
    pub fn new() -> Self {
        Self::for_platform(Platform::current())
    }

    pub fn for_platform(platform: Platform) -> Self {
        Self {
            platform,
            command_timeout: Duration::from_secs(10),
        }
    }

    /// 生成要采取的动作（不执行）。参数不合法或平台不支持时返回 `LAUNCH_FAILED`。
    pub fn action(&self, req: &WakeRequest) -> Result<WakeAction, HubError> {
        self.plan(req)
    }

    fn plan(&self, req: &WakeRequest) -> Result<WakeAction, HubError> {
        let token = &req.token;
        if !is_valid_token(token) {
            return Err(launch_failed("唤醒令牌含非法字符"));
        }
        let target = req.descriptor.target.as_deref().unwrap_or_default();
        let p = self.platform;
        let cmd = |program: &str, args: Vec<String>| {
            WakeAction::Command(WakeCommand {
                program: program.to_owned(),
                args,
            })
        };
        let open_url = |url: String, background: bool| match p {
            Platform::Windows => Ok(cmd(
                "rundll32.exe",
                vec!["url.dll,FileProtocolHandler".into(), url],
            )),
            Platform::MacOs if background => Ok(cmd("open", vec!["-g".into(), url])),
            Platform::MacOs => Ok(cmd("open", vec![url])),
            Platform::Linux => Ok(cmd("xdg-open", vec![url])),
            _ => Err(unsupported(req.descriptor.kind)),
        };
        match req.descriptor.kind {
            WakeKind::Uri => {
                let scheme = target.strip_suffix(':').unwrap_or(target);
                if !is_valid_scheme(scheme) {
                    return Err(launch_failed(format!("唤醒描述中的 URI scheme「{scheme}」不合法")));
                }
                let uri = format!("{scheme}://app-mcp/wake?token={token}");
                match p {
                    // 字符集已校验（无空白、引号、& ^ | < > 等），cmd 不会解释其中任何字符。
                    Platform::Windows => Ok(cmd(
                        "cmd.exe",
                        vec!["/d".into(), "/c".into(), "start".into(), String::new(), uri],
                    )),
                    _ => open_url(uri, true),
                }
            }
            WakeKind::Aumid => {
                if p != Platform::Windows {
                    return Err(unsupported(WakeKind::Aumid));
                }
                if !is_valid_aumid(target) {
                    return Err(launch_failed(format!("AUMID「{target}」不合法")));
                }
                Ok(WakeAction::ActivateApplication {
                    aumid: target.to_owned(),
                    arguments: req.activation_arg.clone(),
                })
            }
            WakeKind::AppleEvent => {
                if p != Platform::MacOs {
                    return Err(unsupported(WakeKind::AppleEvent));
                }
                if !is_valid_bundle_id(target) {
                    return Err(launch_failed(format!("bundle id「{target}」不合法")));
                }
                Ok(cmd(
                    "open",
                    vec![
                        "-g".into(),
                        "-b".into(),
                        target.to_owned(),
                        "--args".into(),
                        req.activation_arg.clone(),
                    ],
                ))
            }
            WakeKind::Dbus => {
                if p != Platform::Linux {
                    return Err(unsupported(WakeKind::Dbus));
                }
                if !is_valid_dbus_name(target) {
                    return Err(launch_failed(format!("D-Bus 名称「{target}」不合法")));
                }
                Ok(cmd(
                    "gdbus",
                    vec![
                        "call".into(),
                        "--session".into(),
                        "--dest".into(),
                        target.to_owned(),
                        "--object-path".into(),
                        dbus_object_path(target),
                        "--method".into(),
                        "org.freedesktop.Application.ActivateAction".into(),
                        "app-mcp-wake".into(),
                        format!("[<'{token}'>]"),
                        "{}".into(),
                    ],
                ))
            }
            WakeKind::WebUrl => {
                if !is_valid_web_url(target) {
                    return Err(launch_failed(format!("网页地址「{target}」不合法")));
                }
                open_url(format!("{target}#app-mcp-wake={token}"), false)
            }
            WakeKind::AndroidIntent => Err(launch_failed(
                "android-intent 唤醒需要由厂商通过 Hub::set_waker 提供实现（默认实现不支持）。",
            )),
            WakeKind::None => Err(launch_failed("该 App 声明为不可唤醒，请让用户手动打开。")),
        }
    }

    async fn perform(&self, action: WakeAction) -> Result<(), HubError> {
        match action {
            WakeAction::Command(c) => self.run(c).await,
            WakeAction::ActivateApplication { aumid, arguments } => {
                tracing::info!(%aumid, %arguments, "ActivateApplication 唤醒");
                let pid = tokio::task::spawn_blocking(move || activate_application(&aumid, &arguments))
                    .await
                    .map_err(|e| launch_failed(format!("激活任务异常结束：{e}")))??;
                tracing::info!(pid, "ActivateApplication 成功");
                Ok(())
            }
        }
    }

    async fn run(&self, c: WakeCommand) -> Result<(), HubError> {
        use std::process::Stdio;
        tracing::info!(program = %c.program, args = ?c.args, "执行唤醒命令");
        let mut command = tokio::process::Command::new(&c.program);
        command
            .args(&c.args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // 常驻 Host 没有控制台：不让 cmd.exe 等控制台程序弹出窗口（被激活的 App 自己的窗口不受影响）。
        #[cfg(windows)]
        command.creation_flags(crate::CREATE_NO_WINDOW);
        let mut child = command
            .spawn()
            .map_err(|e| launch_failed(format!("无法执行 {}：{e}", c.program)))?;
        match tokio::time::timeout(self.command_timeout, child.wait()).await {
            Ok(Ok(status)) if status.success() => Ok(()),
            Ok(Ok(status)) => Err(launch_failed(format!("{} 退出码 {status}", c.program))),
            Ok(Err(e)) => Err(launch_failed(format!("等待 {} 失败：{e}", c.program))),
            // 命令仍在运行（如 xdg-open 等待浏览器）：视为已发出激活。
            Err(_) => Ok(()),
        }
    }
}

#[async_trait::async_trait]
impl Waker for SystemWaker {
    async fn wake(&self, req: WakeRequest) -> Result<(), HubError> {
        let action = self.action(&req)?;
        self.perform(action).await
    }
}

/// Windows：经 `IApplicationActivationManager` 激活打包应用并传入参数，返回被激活进程的 PID。
///
/// 在调用线程上初始化 COM（STA），结束时配对反初始化；线程已处于其他套间模式时沿用现有套间。
#[cfg(windows)]
fn activate_application(aumid: &str, arguments: &str) -> Result<u32, HubError> {
    use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
    use windows::Win32::System::Com::{
        CLSCTX_LOCAL_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
    };
    use windows::Win32::UI::Shell::{AO_NONE, ApplicationActivationManager, IApplicationActivationManager};
    use windows::core::HSTRING;

    struct ComGuard(bool);
    impl Drop for ComGuard {
        fn drop(&mut self) {
            if self.0 {
                // SAFETY：与本线程上成功的 CoInitializeEx 配对。
                unsafe { CoUninitialize() };
            }
        }
    }
    // SAFETY：COM 初始化 / 创建 / 调用均为标准用法；参数是有效的以 NUL 结尾的宽字符串（HSTRING）。
    unsafe {
        let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        if hr.is_err() && hr != RPC_E_CHANGED_MODE {
            return Err(launch_failed(format!("初始化 COM 失败：{hr:?}")));
        }
        let _guard = ComGuard(hr.is_ok());
        let manager: IApplicationActivationManager =
            CoCreateInstance(&ApplicationActivationManager, None, CLSCTX_LOCAL_SERVER)
                .map_err(|e| launch_failed(format!("创建 ApplicationActivationManager 失败：{e}")))?;
        manager
            .ActivateApplication(&HSTRING::from(aumid), &HSTRING::from(arguments), AO_NONE)
            .map_err(|e| launch_failed(format!("激活「{aumid}」失败：{e}")))
    }
}

#[cfg(not(windows))]
fn activate_application(_aumid: &str, _arguments: &str) -> Result<u32, HubError> {
    Err(unsupported(WakeKind::Aumid))
}

fn launch_failed(msg: impl Into<String>) -> HubError {
    HubError::new(ErrorKind::LaunchFailed, msg)
}

fn unsupported(kind: WakeKind) -> HubError {
    launch_failed(format!("当前平台不支持唤醒方式「{}」。", kind_str(kind)))
}

pub(crate) fn kind_str(kind: WakeKind) -> &'static str {
    match kind {
        WakeKind::Uri => "uri",
        WakeKind::Aumid => "aumid",
        WakeKind::AppleEvent => "apple-event",
        WakeKind::Dbus => "dbus",
        WakeKind::AndroidIntent => "android-intent",
        WakeKind::WebUrl => "web-url",
        WakeKind::None => "none",
    }
}

/// 一次性唤醒令牌：128 位随机数的十六进制。
pub(crate) fn new_token() -> String {
    rand::random::<[u8; 16]>()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// `[A-Za-z0-9._~-]{1,512}`（spec/protocol.md 8.3）。
pub fn is_valid_token(t: &str) -> bool {
    !t.is_empty()
        && t.len() <= 512
        && t
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'~' | b'-'))
}

const FORBIDDEN_SCHEMES: &[&str] = &["http", "https", "file", "javascript", "data", "vbscript", "ms-settings"];

fn is_valid_scheme(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && s.len() <= 64
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        && !FORBIDDEN_SCHEMES.contains(&s.to_ascii_lowercase().as_str())
}

fn is_valid_aumid(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 256
        && s.contains('!')
        && s
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'!'))
}

fn is_valid_bundle_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 255
        && !s.starts_with('-')
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'))
}

fn is_valid_dbus_name(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    s.len() <= 255
        && parts.len() >= 2
        && parts.iter().all(|p| {
            let mut c = p.chars();
            c.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                && c.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        })
}

/// `com.example.App` → `/com/example/App`（GApplication 约定，`-` 换成 `_`）。
fn dbus_object_path(name: &str) -> String {
    format!("/{}", name.replace('.', "/").replace('-', "_"))
}

/// http(s) 地址，不含 `#`、空白、控制字符与引号。
fn is_valid_web_url(s: &str) -> bool {
    (s.starts_with("http://") || s.starts_with("https://"))
        && s.len() <= 2048
        && s
            .chars()
            .all(|c| c.is_ascii_graphic() && !matches!(c, '#' | '"' | '\'' | '`' | '\\' | '<' | '>'))
}

/// 清单 `WakeDescriptor` → 协议 `WakeDescriptor`（两者同构）。
fn from_manifest(d: &app_mcp_manifest::WakeDescriptor) -> Option<WakeDescriptor> {
    serde_json::to_value(d)
        .ok()
        .and_then(|v| serde_json::from_value(v).ok())
}

/// 从清单解析唤醒描述（spec/manifest.md 2.2）：显式声明的 `wake.<平台>` → 显式声明的 `wake.web`；
/// `from_launch` 为真时再由 `launch` 推导（`launch.web` 的 `url` → `web-url`；`launch.<平台>` 的
/// `uri` / `aumid` / `bundle` / `dbus`）。
pub(crate) fn manifest_descriptor(m: &Manifest, platform: Platform, from_launch: bool) -> Option<WakeDescriptor> {
    let explicit = |key: &str| m.wake.descriptors(key).next().and_then(from_manifest);
    if let Some(key) = platform.manifest_key()
        && let Some(d) = explicit(key)
    {
        return (d.kind != WakeKind::None).then_some(d);
    }
    if let Some(d) = explicit("web") {
        return (d.kind != WakeKind::None).then_some(d);
    }
    if !from_launch {
        return None;
    }
    let entries: &[LaunchEntry] = match platform {
        Platform::Windows => &m.launch.windows,
        Platform::MacOs => &m.launch.macos,
        Platform::Linux => &m.launch.linux,
        _ => &[],
    };
    let native = entries.iter().find_map(|e| {
        let LaunchEntry::Known(k) = e else { return None };
        let (kind, target) = match k {
            KnownLaunch::Uri { scheme } => (WakeKind::Uri, scheme),
            KnownLaunch::Aumid { id } => (WakeKind::Aumid, id),
            KnownLaunch::Bundle { id } => (WakeKind::AppleEvent, id),
            KnownLaunch::Dbus { name } => (WakeKind::Dbus, name),
            _ => return None,
        };
        Some(WakeDescriptor {
            kind,
            target: Some(target.clone()),
            background: false,
        })
    });
    native.or_else(|| m.wake_descriptor("web").as_ref().and_then(from_manifest))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(kind: WakeKind, target: &str) -> WakeRequest {
        WakeRequest {
            app_id: "shop".into(),
            instance_id: None,
            descriptor: WakeDescriptor {
                kind,
                target: Some(target.into()),
                background: false,
            },
            token: "abc123".into(),
            activation_arg: "app-mcp-wake:abc123".into(),
        }
    }

    fn cmd(p: Platform, kind: WakeKind, target: &str) -> Result<WakeCommand, HubError> {
        match SystemWaker::for_platform(p).action(&req(kind, target))? {
            WakeAction::Command(c) => Ok(c),
            other => panic!("期望命令，实际 {other:?}"),
        }
    }

    #[test]
    fn uri_per_platform() {
        let c = cmd(Platform::Linux, WakeKind::Uri, "shop-app").unwrap();
        assert_eq!((c.program.as_str(), c.args.clone()), ("xdg-open", vec!["shop-app://app-mcp/wake?token=abc123".to_string()]));
        let c = cmd(Platform::MacOs, WakeKind::Uri, "shop-app").unwrap();
        assert_eq!(c.args, vec!["-g", "shop-app://app-mcp/wake?token=abc123"]);
        let c = cmd(Platform::Windows, WakeKind::Uri, "shop-app:").unwrap();
        assert_eq!(c.program, "cmd.exe");
        assert_eq!(c.args, vec!["/d", "/c", "start", "", "shop-app://app-mcp/wake?token=abc123"]);
        for bad in ["http", "JavaScript", "a b", "x&calc", "1abc", ""] {
            assert_eq!(cmd(Platform::Linux, WakeKind::Uri, bad).unwrap_err().kind(), ErrorKind::LaunchFailed, "{bad}");
        }
    }

    #[test]
    fn bad_token_rejected() {
        let mut r = req(WakeKind::Uri, "shop");
        r.token = "a&b".into();
        assert!(SystemWaker::for_platform(Platform::Linux).action(&r).is_err());
        assert!(is_valid_token("A-z_0.9~"));
        assert!(!is_valid_token(""));
        assert!(!is_valid_token("a b"));
    }

    #[test]
    fn aumid_dbus_apple_web() {
        // AUMID：经 ActivateApplication 把令牌作为激活参数传入
        let win = SystemWaker::for_platform(Platform::Windows);
        assert_eq!(
            win.action(&req(WakeKind::Aumid, "Co.Shop_8wekyb3d8bbwe!App")).unwrap(),
            WakeAction::ActivateApplication {
                aumid: "Co.Shop_8wekyb3d8bbwe!App".into(),
                arguments: "app-mcp-wake:abc123".into(),
            }
        );
        assert!(win.action(&req(WakeKind::Aumid, "x & calc!App")).is_err());
        assert!(SystemWaker::for_platform(Platform::Linux).action(&req(WakeKind::Aumid, "Co.Shop!App")).is_err());

        let c = cmd(Platform::Linux, WakeKind::Dbus, "com.example.My-Shop").unwrap();
        assert_eq!(c.program, "gdbus");
        assert!(c.args.contains(&"/com/example/My_Shop".to_string()));
        assert!(c.args.contains(&"[<'abc123'>]".to_string()));
        assert!(cmd(Platform::Linux, WakeKind::Dbus, "noDots").is_err());
        assert!(cmd(Platform::Linux, WakeKind::Dbus, "com.x;rm").is_err());

        let c = cmd(Platform::MacOs, WakeKind::AppleEvent, "com.example.shop").unwrap();
        assert_eq!(c.args, vec!["-g", "-b", "com.example.shop", "--args", "app-mcp-wake:abc123"]);
        assert!(cmd(Platform::MacOs, WakeKind::AppleEvent, "-x").is_err());

        let c = cmd(Platform::Windows, WakeKind::WebUrl, "http://localhost:5173/?a=1&b=2").unwrap();
        assert_eq!(c.program, "rundll32.exe");
        assert_eq!(c.args[1], "http://localhost:5173/?a=1&b=2#app-mcp-wake=abc123");
        let c = cmd(Platform::Linux, WakeKind::WebUrl, "https://x.test/").unwrap();
        assert_eq!(c.args, vec!["https://x.test/#app-mcp-wake=abc123"]);
        assert!(cmd(Platform::Linux, WakeKind::WebUrl, "https://x.test/#f").is_err());
        assert!(cmd(Platform::Linux, WakeKind::WebUrl, "file:///etc/passwd").is_err());
        assert!(cmd(Platform::Linux, WakeKind::WebUrl, "http://a b").is_err());

        assert!(cmd(Platform::Linux, WakeKind::AndroidIntent, "a/b").is_err());
        assert!(cmd(Platform::Linux, WakeKind::None, "").is_err());
    }

    #[test]
    fn manifest_resolution() {
        let m = app_mcp_manifest::parse(
            r#"{"manifestVersion":1,"appId":"shop","name":"S",
                "launch":{"web":[{"type":"url","href":"http://localhost:5173/"}],"linux":[{"type":"dbus","name":"com.x.Shop"}]},
                "wake":{"windows":[{"kind":"uri","target":"shop-app"}]}}"#,
        )
        .unwrap();
        let d = manifest_descriptor(&m, Platform::Windows, false).unwrap();
        assert_eq!((d.kind, d.target.as_deref()), (WakeKind::Uri, Some("shop-app")));
        // Linux：没有显式 wake；不从 launch 推导时不可唤醒
        assert!(manifest_descriptor(&m, Platform::Linux, false).is_none());
        // 从 launch 推导：launch.linux 优先于由 launch.web 推导的 web-url
        let d = manifest_descriptor(&m, Platform::Linux, true).unwrap();
        assert_eq!((d.kind, d.target.as_deref()), (WakeKind::Dbus, Some("com.x.Shop")));
        let d = manifest_descriptor(&m, Platform::MacOs, true).unwrap();
        assert_eq!(d.kind, WakeKind::WebUrl);
        let m2 = app_mcp_manifest::parse(
            r#"{"manifestVersion":1,"appId":"shop","name":"S","launch":{"linux":[{"type":"dbus","name":"com.x.Shop"}]}}"#,
        )
        .unwrap();
        let d = manifest_descriptor(&m2, Platform::Linux, true).unwrap();
        assert_eq!((d.kind, d.target.as_deref()), (WakeKind::Dbus, Some("com.x.Shop")));
        assert!(manifest_descriptor(&m2, Platform::MacOs, true).is_none());
        let m3 = app_mcp_manifest::parse(
            r#"{"manifestVersion":1,"appId":"shop","name":"S","wake":{"linux":[{"kind":"none"}],"web":[{"kind":"web-url","target":"http://localhost:1/"}]}}"#,
        )
        .unwrap();
        assert!(manifest_descriptor(&m3, Platform::Linux, true).is_none());
        assert_eq!(manifest_descriptor(&m3, Platform::Windows, false).unwrap().kind, WakeKind::WebUrl);
    }

    #[tokio::test]
    async fn run_reports_missing_program() {
        let w = SystemWaker::for_platform(Platform::Linux);
        let e = w
            .run(WakeCommand {
                program: "definitely-not-a-program-app-mcp".into(),
                args: vec![],
            })
            .await
            .unwrap_err();
        assert_eq!(e.kind(), ErrorKind::LaunchFailed);
        #[cfg(unix)]
        {
            assert!(w.run(WakeCommand { program: "true".into(), args: vec![] }).await.is_ok());
            assert!(w.run(WakeCommand { program: "false".into(), args: vec![] }).await.is_err());
        }
    }

    /// Windows 实机：以计算器的 AUMID 经 IApplicationActivationManager 激活，返回 PID 后结束该进程。
    /// 会在桌面上短暂打开计算器，默认忽略：`cargo test -p app-mcp-hub -- --ignored activate_calculator`。
    ///
    /// 注：计算器自身不接受启动参数（带参数激活时由计算器返回 0x8004090x 并退出），因此这里用空参数验证
    /// 激活链路；令牌参数的送达由接入 SDK 的打包 App 处理（`handleWake`）。
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn activate_calculator() {
        let pid = activate_application("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App", "")
            .expect("ActivateApplication 应成功");
        assert!(pid > 0);
        let status = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .status()
            .expect("taskkill");
        assert!(status.success(), "结束计算器进程 {pid} 失败");
        let e = activate_application("NoSuch.App_0000000000000!App", "app-mcp-wake:abc123").unwrap_err();
        assert_eq!(e.kind(), ErrorKind::LaunchFailed);
    }
}
