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
    /// 为 `true` 时忽略退出码（`explorer.exe` 成功时也常返回 1）。
    pub ignore_exit_code: bool,
}

/// 默认唤醒实现：按平台执行系统命令。
///
/// | kind | Windows | macOS | Linux |
/// |---|---|---|---|
/// | `uri` | `cmd /c start "" <scheme>://app-mcp/wake?token=<t>` | `open -g <uri>` | `xdg-open <uri>` |
/// | `aumid` | `explorer.exe shell:AppsFolder\<aumid>`（不能传参，App 激活后需自行 `wake()`）| — | — |
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

    /// 生成要执行的命令（不执行）。参数不合法或平台不支持时返回 `LAUNCH_FAILED`。
    pub fn command(&self, req: &WakeRequest) -> Result<WakeCommand, HubError> {
        let token = &req.token;
        if !is_valid_token(token) {
            return Err(launch_failed("唤醒令牌含非法字符"));
        }
        let target = req.descriptor.target.as_deref().unwrap_or_default();
        let p = self.platform;
        let cmd = |program: &str, args: Vec<String>| WakeCommand {
            program: program.to_owned(),
            args,
            ignore_exit_code: false,
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
                Ok(WakeCommand {
                    program: "explorer.exe".into(),
                    args: vec![format!("shell:AppsFolder\\{target}")],
                    ignore_exit_code: true,
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
            Ok(Ok(status)) if status.success() || c.ignore_exit_code => Ok(()),
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
        let c = self.command(&req)?;
        self.run(c).await
    }
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
        SystemWaker::for_platform(p).command(&req(kind, target))
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
        assert!(SystemWaker::for_platform(Platform::Linux).command(&r).is_err());
        assert!(is_valid_token("A-z_0.9~"));
        assert!(!is_valid_token(""));
        assert!(!is_valid_token("a b"));
    }

    #[test]
    fn aumid_dbus_apple_web() {
        let c = cmd(Platform::Windows, WakeKind::Aumid, "Co.Shop_8wekyb3d8bbwe!App").unwrap();
        assert_eq!(c.program, "explorer.exe");
        assert_eq!(c.args, vec!["shell:AppsFolder\\Co.Shop_8wekyb3d8bbwe!App"]);
        assert!(c.ignore_exit_code);
        assert!(cmd(Platform::Windows, WakeKind::Aumid, "x & calc!App").is_err());
        assert!(cmd(Platform::Linux, WakeKind::Aumid, "Co.Shop!App").is_err());

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
                ignore_exit_code: false,
            })
            .await
            .unwrap_err();
        assert_eq!(e.kind(), ErrorKind::LaunchFailed);
        #[cfg(unix)]
        {
            assert!(w.run(WakeCommand { program: "true".into(), args: vec![], ignore_exit_code: false }).await.is_ok());
            assert!(w.run(WakeCommand { program: "false".into(), args: vec![], ignore_exit_code: false }).await.is_err());
            assert!(w.run(WakeCommand { program: "false".into(), args: vec![], ignore_exit_code: true }).await.is_ok());
        }
    }
}
