//! 配置目录与配置文件（`~/.app-mcp/config.json`）。
//!
//! 优先级：命令行参数 > 配置文件 > 默认值。配置目录：`--home` > 环境变量 `APP_MCP_HOME` > `~/.app-mcp`。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use app_mcp_hub::{ToolExposure, UpstreamConfig, WakerConfig};
use serde::{Deserialize, Serialize};

/// 默认的 App 连接服务（WebSocket）地址。
pub const DEFAULT_WS_ADDR: &str = app_mcp_protocol::DEFAULT_WS_ADDR;
/// 默认的 MCP Streamable HTTP 监听地址。
pub const DEFAULT_HTTP_ADDR: &str = "127.0.0.1:7718";
/// 配置目录环境变量。
pub const HOME_ENV: &str = "APP_MCP_HOME";

/// 配置目录（`~/.app-mcp`）及其中的固定文件。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppHome {
    pub dir: PathBuf,
}

impl AppHome {
    /// `--home` > `APP_MCP_HOME` > `~/.app-mcp`。结果为绝对路径。
    pub fn resolve(explicit: Option<&Path>) -> anyhow::Result<Self> {
        let dir = match explicit {
            Some(d) => d.to_path_buf(),
            None => match std::env::var_os(HOME_ENV).filter(|v| !v.is_empty()) {
                Some(d) => PathBuf::from(d),
                None => dirs::home_dir()
                    .context("无法确定用户主目录；请用 --home 或 APP_MCP_HOME 指定配置目录")?
                    .join(".app-mcp"),
            },
        };
        Ok(Self {
            dir: absolute(&dir)?,
        })
    }

    pub fn config_file(&self) -> PathBuf {
        self.dir.join("config.json")
    }
    pub fn token_file(&self) -> PathBuf {
        self.dir.join("token")
    }
    pub fn log_dir(&self) -> PathBuf {
        self.dir.join("logs")
    }
    pub fn manifest_dir(&self) -> PathBuf {
        self.dir.join("manifests")
    }
}

/// 转为绝对路径（不要求存在），并展开开头的 `~/`。
pub fn absolute(p: &Path) -> anyhow::Result<PathBuf> {
    let p = expand_tilde(p);
    if p.is_absolute() {
        return Ok(p);
    }
    Ok(std::env::current_dir().context("无法读取当前目录")?.join(p))
}

fn expand_tilde(p: &Path) -> PathBuf {
    let Some(s) = p.to_str() else {
        return p.to_path_buf();
    };
    let rest = if s == "~" {
        Some("")
    } else {
        s.strip_prefix("~/").or_else(|| s.strip_prefix("~\\"))
    };
    match (rest, dirs::home_dir()) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => p.to_path_buf(),
    }
}

/// HTTP 端点的令牌策略（见 README「安全」）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum AuthMode {
    /// 默认：带 `Origin` 头的请求（浏览器）必须携带令牌；本地非浏览器客户端可不带。
    #[default]
    Browser,
    /// 所有请求都必须携带令牌（多用户共享的机器建议使用）。
    All,
    /// 不使用令牌（只做 Origin / Host 校验）。
    Off,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct HttpSection {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub addr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_remote: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth: Option<AuthMode>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LifecycleSection {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lease_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wake_timeout_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wake_from_launch: Option<bool>,
    /// `"system"`（默认）/ `"none"` / `{"exec": [program, ...args]}`（spec/hub-api.md 3.5）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waker: Option<WakerConfig>,
}

/// 工具列表（spec/hub-api.md 3.7）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ToolsSection {
    /// `"auto"`（默认）/ `"progressive"` / `"all"`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exposure: Option<ToolExposure>,
    /// `auto` 的阈值（App 与上游工具总数超过它时渐进暴露），默认 40。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threshold: Option<usize>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LogSection {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    /// 常驻模式是否写日志文件（`<home>/logs/app-mcp-host.log`），默认 `true`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<bool>,
    /// 单个日志文件上限（字节），超过后轮转；默认 5 MiB。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u64>,
    /// 保留的历史文件数（`.1` … `.N`），默认 3。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keep: Option<usize>,
}

/// `config.json` 的内容。所有字段都可省略；兼容旧的 `--config` 文件（只有 `upstreams`）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FileConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ws_addr: Option<String>,
    #[serde(skip_serializing_if = "is_default")]
    pub http: HttpSection,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub manifests: Vec<PathBuf>,
    /// 清单目录；省略时为 `<home>/manifests`（不存在时忽略）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest_dirs: Option<Vec<PathBuf>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub allow_origins: Vec<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub upstreams: BTreeMap<String, UpstreamConfig>,
    #[serde(skip_serializing_if = "is_default")]
    pub lifecycle: LifecycleSection,
    #[serde(skip_serializing_if = "is_default")]
    pub tools: ToolsSection,
    #[serde(skip_serializing_if = "is_default")]
    pub log: LogSection,
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

impl FileConfig {
    /// 读取配置文件。`must_exist = false` 时文件不存在返回默认配置。
    pub fn load(path: &Path, must_exist: bool) -> anyhow::Result<Self> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if !must_exist && e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(e) => return Err(anyhow::anyhow!("读取配置 {} 失败：{e}", path.display())),
        };
        serde_json::from_str(&text).with_context(|| format!("解析配置 {} 失败", path.display()))
    }

    /// 写入配置文件（格式化 JSON），必要时创建目录。
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("创建目录 {} 失败", parent.display()))?;
        }
        let mut text = serde_json::to_string_pretty(self)?;
        text.push('\n');
        std::fs::write(path, text).with_context(|| format!("写入配置 {} 失败", path.display()))
    }
}

/// 命令行给出的覆盖项（`None` / 空 = 未指定，沿用配置文件）。
#[derive(Clone, Debug, Default)]
pub struct Overrides {
    pub ws_addr: Option<String>,
    pub http_addr: Option<String>,
    pub http_allow_remote: Option<bool>,
    pub auth: Option<AuthMode>,
    pub manifests: Vec<PathBuf>,
    pub manifest_dirs: Vec<PathBuf>,
    pub allow_origins: Vec<String>,
    pub upstreams: BTreeMap<String, UpstreamConfig>,
    pub lease_ms: Option<u64>,
    pub wake_timeout_ms: Option<u64>,
    pub wake_from_launch: Option<bool>,
    pub waker: Option<WakerConfig>,
    pub tool_exposure: Option<ToolExposure>,
    pub tool_exposure_threshold: Option<usize>,
    pub log_level: Option<String>,
    pub log_file: Option<bool>,
}

impl FileConfig {
    /// 把命令行覆盖项写入配置（`service install` 用于持久化；`serve` 用于合并）。
    /// 标量覆盖；列表 / 映射追加（同名上游被覆盖）。路径转为绝对路径。
    pub fn apply(&mut self, o: &Overrides) -> anyhow::Result<()> {
        fn set<T: Clone>(dst: &mut Option<T>, src: &Option<T>) {
            if let Some(v) = src {
                *dst = Some(v.clone());
            }
        }
        set(&mut self.ws_addr, &o.ws_addr);
        set(&mut self.http.addr, &o.http_addr);
        set(&mut self.http.allow_remote, &o.http_allow_remote);
        set(&mut self.http.auth, &o.auth);
        set(&mut self.lifecycle.lease_ms, &o.lease_ms);
        set(&mut self.lifecycle.wake_timeout_ms, &o.wake_timeout_ms);
        set(&mut self.lifecycle.wake_from_launch, &o.wake_from_launch);
        set(&mut self.lifecycle.waker, &o.waker);
        set(&mut self.tools.exposure, &o.tool_exposure);
        set(&mut self.tools.threshold, &o.tool_exposure_threshold);
        set(&mut self.log.level, &o.log_level);
        set(&mut self.log.file, &o.log_file);
        for m in &o.manifests {
            let m = absolute(m)?;
            if !self.manifests.contains(&m) {
                self.manifests.push(m);
            }
        }
        if !o.manifest_dirs.is_empty() {
            let dirs = o
                .manifest_dirs
                .iter()
                .map(|d| absolute(d))
                .collect::<anyhow::Result<Vec<_>>>()?;
            self.manifest_dirs = Some(dirs);
        }
        for origin in &o.allow_origins {
            if !self.allow_origins.contains(origin) {
                self.allow_origins.push(origin.clone());
            }
        }
        for (name, cfg) in &o.upstreams {
            self.upstreams.insert(name.clone(), cfg.clone());
        }
        Ok(())
    }
}

/// 合并后的最终设置。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub ws_addr: String,
    pub http_addr: String,
    pub http_allow_remote: bool,
    pub auth: AuthMode,
    pub manifests: Vec<PathBuf>,
    /// `(目录, 是否必须存在)`。显式配置的目录必须存在；默认目录不存在时忽略。
    pub manifest_dirs: Vec<(PathBuf, bool)>,
    pub allow_origins: Vec<String>,
    pub upstreams: BTreeMap<String, UpstreamConfig>,
    pub lease_ms: u64,
    pub wake_timeout_ms: u64,
    pub wake_from_launch: bool,
    pub waker: WakerConfig,
    pub tool_exposure: ToolExposure,
    pub tool_exposure_threshold: usize,
    pub log_level: String,
    pub log_file: bool,
    pub log_max_bytes: u64,
    pub log_keep: usize,
}

impl Settings {
    /// 配置文件 + 命令行覆盖 → 最终设置。
    pub fn resolve(file: &FileConfig, o: &Overrides, home: &AppHome) -> anyhow::Result<Self> {
        let mut c = file.clone();
        c.apply(o)?;
        let manifest_dirs = match c.manifest_dirs {
            Some(dirs) => dirs
                .iter()
                .map(|d| absolute(d).map(|d| (d, true)))
                .collect::<anyhow::Result<_>>()?,
            None => vec![(home.manifest_dir(), false)],
        };
        let manifests = c
            .manifests
            .iter()
            .map(|m| absolute(m))
            .collect::<anyhow::Result<_>>()?;
        Ok(Self {
            ws_addr: c.ws_addr.unwrap_or_else(|| DEFAULT_WS_ADDR.to_owned()),
            http_addr: c.http.addr.unwrap_or_else(|| DEFAULT_HTTP_ADDR.to_owned()),
            http_allow_remote: c.http.allow_remote.unwrap_or(false),
            auth: c.http.auth.unwrap_or_default(),
            manifests,
            manifest_dirs,
            allow_origins: c.allow_origins,
            upstreams: c.upstreams,
            lease_ms: c.lifecycle.lease_ms.unwrap_or(60_000),
            wake_timeout_ms: c.lifecycle.wake_timeout_ms.unwrap_or(15_000),
            wake_from_launch: c.lifecycle.wake_from_launch.unwrap_or(false),
            waker: c.lifecycle.waker.unwrap_or_default(),
            tool_exposure: c.tools.exposure.unwrap_or_default(),
            tool_exposure_threshold: c
                .tools
                .threshold
                .unwrap_or(app_mcp_hub::DEFAULT_TOOL_EXPOSURE_THRESHOLD),
            log_level: c.log.level.unwrap_or_else(|| "info".to_owned()),
            log_file: c.log.file.unwrap_or(true),
            log_max_bytes: c.log.max_bytes.unwrap_or(5 * 1024 * 1024),
            log_keep: c.log.keep.unwrap_or(3),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn abs(p: &str) -> PathBuf {
        absolute(Path::new(p)).unwrap()
    }

    fn home() -> AppHome {
        AppHome {
            dir: PathBuf::from("/h/.app-mcp"),
        }
    }

    #[test]
    fn defaults() {
        let s = Settings::resolve(&FileConfig::default(), &Overrides::default(), &home()).unwrap();
        assert_eq!(s.ws_addr, "127.0.0.1:7717");
        assert_eq!(s.http_addr, "127.0.0.1:7718");
        assert_eq!(s.auth, AuthMode::Browser);
        assert_eq!(
            s.manifest_dirs,
            vec![(PathBuf::from("/h/.app-mcp/manifests"), false)]
        );
        assert!(s.log_file);
        assert_eq!(s.lease_ms, 60_000);
        assert_eq!(s.waker, WakerConfig::System);
        assert_eq!(s.tool_exposure, ToolExposure::Auto);
        assert_eq!(s.tool_exposure_threshold, 40);
    }

    #[test]
    fn parse_full_and_legacy() {
        let full: FileConfig = serde_json::from_str(
            r#"{
              "wsAddr": "127.0.0.1:9000",
              "http": { "addr": "127.0.0.1:9001", "auth": "all" },
              "manifests": ["/m/a.json"],
              "manifestDirs": ["/m"],
              "allowOrigins": ["https://app.example.com"],
              "upstreams": { "files": { "command": "npx", "args": ["x"] } },
              "lifecycle": { "leaseMs": 500, "wakeTimeoutMs": 2000, "wakeFromLaunch": true,
                             "waker": { "exec": ["node", "wake.mjs"] } },
              "tools": { "exposure": "progressive", "threshold": 10 },
              "log": { "level": "debug", "file": false, "maxBytes": 1024, "keep": 1 }
            }"#,
        )
        .unwrap();
        let s = Settings::resolve(&full, &Overrides::default(), &home()).unwrap();
        assert_eq!(s.ws_addr, "127.0.0.1:9000");
        assert_eq!(s.http_addr, "127.0.0.1:9001");
        assert_eq!(s.auth, AuthMode::All);
        // Windows 上 "/m" 不是绝对路径，会接到当前目录（盘符）上
        assert_eq!(s.manifest_dirs, vec![(abs("/m"), true)]);
        assert_eq!(s.upstreams["files"].command, "npx");
        assert_eq!(
            (s.lease_ms, s.wake_timeout_ms, s.wake_from_launch),
            (500, 2000, true)
        );
        assert_eq!(
            s.waker,
            WakerConfig::Exec(vec!["node".into(), "wake.mjs".into()])
        );
        assert_eq!(
            (s.tool_exposure, s.tool_exposure_threshold),
            (ToolExposure::Progressive, 10)
        );
        assert_eq!(
            (
                s.log_level.as_str(),
                s.log_file,
                s.log_max_bytes,
                s.log_keep
            ),
            ("debug", false, 1024, 1)
        );

        // 旧的 --config 文件（只有 upstreams）仍可解析
        let legacy: FileConfig =
            serde_json::from_str(r#"{"upstreams":{"e":{"command":"echo"}}}"#).unwrap();
        assert_eq!(legacy.upstreams.len(), 1);
    }

    #[test]
    fn cli_overrides_file() {
        let file: FileConfig = serde_json::from_str(
            r#"{"wsAddr":"127.0.0.1:9000","manifests":["/m/a.json"],"lifecycle":{"leaseMs":500}}"#,
        )
        .unwrap();
        let o = Overrides {
            ws_addr: Some("127.0.0.1:1".into()),
            manifests: vec![PathBuf::from("/m/b.json")],
            auth: Some(AuthMode::Off),
            ..Default::default()
        };
        let s = Settings::resolve(&file, &o, &home()).unwrap();
        assert_eq!(s.ws_addr, "127.0.0.1:1");
        assert_eq!(s.manifests, vec![abs("/m/a.json"), abs("/m/b.json")]);
        assert_eq!(s.lease_ms, 500);
        assert_eq!(s.auth, AuthMode::Off);
    }

    #[test]
    fn save_roundtrip_skips_empty() {
        let dir = std::env::temp_dir().join(format!("app-mcp-cfg-{}", std::process::id()));
        let path = dir.join("config.json");
        let mut c = FileConfig::default();
        c.apply(&Overrides {
            http_addr: Some("127.0.0.1:7718".into()),
            ..Default::default()
        })
        .unwrap();
        c.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            text,
            "{\n  \"http\": {\n    \"addr\": \"127.0.0.1:7718\"\n  }\n}\n"
        );
        assert_eq!(FileConfig::load(&path, true).unwrap(), c);
        assert_eq!(
            FileConfig::load(&dir.join("missing.json"), false).unwrap(),
            FileConfig::default()
        );
        assert!(FileConfig::load(&dir.join("missing.json"), true).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
