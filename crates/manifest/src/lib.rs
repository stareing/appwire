//! 静态能力清单 `app-mcp.json`（规范见 `spec/manifest.md`）。
//!
//! - [`parse`]：从 JSON 文本解析 [`Manifest`]（只做结构解析，不做语义校验）。
//! - [`Manifest::validate`]：按规范第 3 节校验，返回错误与警告列表。
//! - [`load_file`] / [`load_dir`]：读取文件并解析 + 校验；目录中读取所有 `*.json`。
//!
//! 未知的 launch `type` 会原样保留（[`LaunchEntry::Other`]），校验时只给出警告。
//! `wake`（唤醒描述，spec/lifecycle.md 第 5 节）同理：未知 `kind` 保留在 [`WakeEntry::Other`]。

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};

pub use app_mcp_protocol::{AppOverview, ResourceInfo, ToolInfo};
use app_mcp_protocol::{
    OVERVIEW_BODY_MAX_CHARS, OVERVIEW_SUMMARY_MAX_CHARS, is_valid_app_id, is_valid_name,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 当前支持的清单版本。
pub const MANIFEST_VERSION: u64 = 1;

/// 保留的 appId：被 Host 内置工具或未来的系统能力占用。
pub const RESERVED_APP_IDS: &[&str] = &["apps", "os", "ax", "host"];

/// 判断 appId 是否为保留名。
pub fn is_reserved_app_id(id: &str) -> bool {
    RESERVED_APP_IDS.contains(&id)
}

/// 静态能力清单。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub manifest_version: u64,
    pub app_id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// App 总览（spec/protocol.md 第 7 节）；运行时 `app/hello` 中的总览优先。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overview: Option<AppOverview>,
    #[serde(default, skip_serializing_if = "Launch::is_empty")]
    pub launch: Launch,
    /// 各平台唤醒描述（spec/lifecycle.md 第 5 节）；Host 唤醒休眠 / 未运行的 App 时优先于 `launch`。
    #[serde(default, skip_serializing_if = "Wake::is_empty")]
    pub wake: Wake,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolInfo>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resources: Vec<ResourceInfo>,
}

/// 各平台唤醒方式，值为按顺序尝试的数组。未知平台键原样保留在 `other` 中。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Launch {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub web: Vec<LaunchEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub windows: Vec<LaunchEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub macos: Vec<LaunchEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub linux: Vec<LaunchEntry>,
    /// 未知平台（向后兼容，原样透传）。
    #[serde(flatten)]
    pub other: BTreeMap<String, Value>,
}

impl Launch {
    pub fn is_empty(&self) -> bool {
        self.web.is_empty()
            && self.windows.is_empty()
            && self.macos.is_empty()
            && self.linux.is_empty()
            && self.other.is_empty()
    }
}

/// 一条唤醒方式。已知类型解析为 [`KnownLaunch`]，其他（未知 `type` 或字段不完整）原样保留。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum LaunchEntry {
    Known(KnownLaunch),
    Other(Value),
}

/// 规范第 2.1 节定义的唤醒方式。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum KnownLaunch {
    /// 网页地址（web）。
    Url { href: String },
    /// 自定义 URI 协议（windows / macos / linux）。
    Uri { scheme: String },
    /// Windows 应用用户模型 ID。
    Aumid { id: String },
    /// 可执行文件路径（可含环境变量）。
    Exe { path: String },
    /// macOS bundle id。
    Bundle { id: String },
    /// Linux D-Bus 名称。
    Dbus { name: String },
    /// Linux .desktop 文件。
    Desktop { file: String },
}

impl KnownLaunch {
    pub fn type_name(&self) -> &'static str {
        match self {
            KnownLaunch::Url { .. } => "url",
            KnownLaunch::Uri { .. } => "uri",
            KnownLaunch::Aumid { .. } => "aumid",
            KnownLaunch::Exe { .. } => "exe",
            KnownLaunch::Bundle { .. } => "bundle",
            KnownLaunch::Dbus { .. } => "dbus",
            KnownLaunch::Desktop { .. } => "desktop",
        }
    }

    /// 该类型适用的平台。
    fn platforms(&self) -> &'static [&'static str] {
        match self {
            KnownLaunch::Url { .. } => &["web"],
            KnownLaunch::Uri { .. } => &["windows", "macos", "linux"],
            KnownLaunch::Aumid { .. } => &["windows"],
            KnownLaunch::Exe { .. } => &["windows", "linux"],
            KnownLaunch::Bundle { .. } => &["macos"],
            KnownLaunch::Dbus { .. } | KnownLaunch::Desktop { .. } => &["linux"],
        }
    }
}

const KNOWN_LAUNCH_TYPES: &[&str] = &["url", "uri", "aumid", "exe", "bundle", "dbus", "desktop"];

/// 清单 `wake` 字段：各平台的唤醒描述，值为按顺序尝试的数组。未知平台键原样保留在 `other` 中。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Wake {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub web: Vec<WakeEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub windows: Vec<WakeEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub macos: Vec<WakeEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub linux: Vec<WakeEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub android: Vec<WakeEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ios: Vec<WakeEntry>,
    /// 未知平台（向后兼容，原样透传）。
    #[serde(flatten)]
    pub other: BTreeMap<String, Value>,
}

/// `wake` 已知的平台键。
pub const WAKE_PLATFORMS: &[&str] = &["web", "windows", "macos", "linux", "android", "ios"];

impl Wake {
    pub fn is_empty(&self) -> bool {
        self.web.is_empty()
            && self.windows.is_empty()
            && self.macos.is_empty()
            && self.linux.is_empty()
            && self.android.is_empty()
            && self.ios.is_empty()
            && self.other.is_empty()
    }

    /// 某个已知平台的条目；未知平台返回空切片。
    pub fn platform(&self, platform: &str) -> &[WakeEntry] {
        match platform {
            "web" => &self.web,
            "windows" => &self.windows,
            "macos" => &self.macos,
            "linux" => &self.linux,
            "android" => &self.android,
            "ios" => &self.ios,
            _ => &[],
        }
    }

    /// 某个平台上按顺序可用的唤醒描述（跳过无法识别的条目）。
    pub fn descriptors<'a>(
        &'a self,
        platform: &str,
    ) -> impl Iterator<Item = &'a WakeDescriptor> + 'a {
        self.platform(platform).iter().filter_map(|e| match e {
            WakeEntry::Known(d) => Some(d),
            WakeEntry::Other(_) => None,
        })
    }
}

/// 一条唤醒描述。已知 `kind` 解析为 [`WakeDescriptor`]，其他（未知 `kind` 或字段类型错误）原样保留。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WakeEntry {
    Known(WakeDescriptor),
    Other(Value),
}

/// 唤醒描述（与 spec/lifecycle.md 第 5 节的 WakeDescriptor 同构）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WakeDescriptor {
    pub kind: WakeKind,
    /// 各 kind 的定位信息；`none` 不需要。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// 能否不把窗口带到前台就唤醒。缺省 false。
    #[serde(default, skip_serializing_if = "is_false")]
    pub background: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// 唤醒方式。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WakeKind {
    /// 自定义 URI 协议：`target` 为 scheme（如 `shop-app`），Host 打开 `<scheme>://app-mcp/wake?token=…`。
    Uri,
    /// Windows 应用用户模型 ID：`target` 为 AUMID。
    Aumid,
    /// macOS Apple Event：`target` 为 bundle id。
    AppleEvent,
    /// Linux D-Bus：`target` 为可激活的 well-known 名称（如 `com.company.Shop`）。
    Dbus,
    /// Android 显式广播：`target` 为 `<包名>/<接收器类名>`。
    AndroidIntent,
    /// 网页：`target` 为 http(s) 地址，Host 打开时附加 `#app-mcp-wake=<token>`。
    WebUrl,
    /// 明确声明不可唤醒。
    None,
}

impl WakeKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            WakeKind::Uri => "uri",
            WakeKind::Aumid => "aumid",
            WakeKind::AppleEvent => "apple-event",
            WakeKind::Dbus => "dbus",
            WakeKind::AndroidIntent => "android-intent",
            WakeKind::WebUrl => "web-url",
            WakeKind::None => "none",
        }
    }

    /// 该方式适用的平台。
    pub fn platforms(&self) -> &'static [&'static str] {
        match self {
            WakeKind::Uri => &["windows", "macos", "linux", "android", "ios"],
            WakeKind::Aumid => &["windows"],
            WakeKind::AppleEvent => &["macos"],
            WakeKind::Dbus => &["linux"],
            WakeKind::AndroidIntent => &["android"],
            WakeKind::WebUrl => &["web"],
            WakeKind::None => WAKE_PLATFORMS,
        }
    }
}

const KNOWN_WAKE_KINDS: &[&str] = &[
    "uri",
    "aumid",
    "apple-event",
    "dbus",
    "android-intent",
    "web-url",
    "none",
];

fn is_valid_uri_scheme(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// 一条校验问题。`path` 为 JSON 路径（如 `tools[1].name`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Issue {
    pub path: String,
    pub message: String,
}

impl Issue {
    fn new(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.path.is_empty() {
            f.write_str(&self.message)
        } else {
            write!(f, "{}: {}", self.path, self.message)
        }
    }
}

/// 校验结果：有错误时清单不可用；警告不影响使用。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Validation {
    pub errors: Vec<Issue>,
    pub warnings: Vec<Issue>,
}

impl Validation {
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
}

impl Manifest {
    /// 按 `spec/manifest.md` 第 3 节校验。
    pub fn validate(&self) -> Validation {
        let mut v = Validation::default();

        if self.manifest_version != MANIFEST_VERSION {
            v.errors.push(Issue::new(
                "manifestVersion",
                format!(
                    "不支持的清单版本 {}，当前只支持 {MANIFEST_VERSION}",
                    self.manifest_version
                ),
            ));
        }
        if !is_valid_app_id(&self.app_id) {
            v.errors.push(Issue::new(
                "appId",
                format!(
                    "appId \"{}\" 格式不合法，应满足 [a-z][a-z0-9-]{{0,62}}",
                    self.app_id
                ),
            ));
        } else if is_reserved_app_id(&self.app_id) {
            v.errors.push(Issue::new(
                "appId",
                format!("appId \"{}\" 是保留名", self.app_id),
            ));
        }
        if self.name.trim().is_empty() {
            v.warnings.push(Issue::new("name", "name 为空"));
        }
        if self.description.as_deref() == Some("") {
            v.errors
                .push(Issue::new("description", "description 不能为空字符串"));
        }
        if let Some(ov) = &self.overview {
            if ov.summary.trim().is_empty() {
                v.errors
                    .push(Issue::new("overview.summary", "overview.summary 不能为空"));
            }
            let n = ov.summary.chars().count();
            if n > OVERVIEW_SUMMARY_MAX_CHARS {
                v.warnings.push(Issue::new(
                    "overview.summary",
                    format!(
                        "summary 有 {n} 个字符，超过 {OVERVIEW_SUMMARY_MAX_CHARS}，Host 会截断"
                    ),
                ));
            }
            if let Some(body) = &ov.body {
                let n = body.chars().count();
                if n > OVERVIEW_BODY_MAX_CHARS {
                    v.warnings.push(Issue::new(
                        "overview.body",
                        format!("body 有 {n} 个字符，超过 {OVERVIEW_BODY_MAX_CHARS}，Host 会截断"),
                    ));
                }
            }
        }

        let mut seen = HashSet::new();
        for (i, tool) in self.tools.iter().enumerate() {
            let path = format!("tools[{i}]");
            if !is_valid_name(&tool.name) {
                v.errors.push(Issue::new(
                    format!("{path}.name"),
                    format!(
                        "工具名 \"{}\" 不合法，应满足 [a-zA-Z0-9_.-]{{1,64}}",
                        tool.name
                    ),
                ));
            } else if !seen.insert(tool.name.as_str()) {
                v.errors.push(Issue::new(
                    format!("{path}.name"),
                    format!("工具名 \"{}\" 重复", tool.name),
                ));
            }
            if app_mcp_protocol::has_app_id_prefix(&tool.name, &self.app_id) {
                v.warnings.push(Issue::new(
                    format!("{path}.name"),
                    app_mcp_protocol::app_id_prefix_warning(&tool.name, &self.app_id),
                ));
            }
            if tool.description.is_empty() {
                v.errors.push(Issue::new(
                    format!("{path}.description"),
                    "description 不能为空字符串",
                ));
            }
            match tool.input_schema.as_object() {
                None => v.errors.push(Issue::new(
                    format!("{path}.inputSchema"),
                    "inputSchema 必须是对象",
                )),
                Some(obj) => {
                    if obj.get("type").and_then(Value::as_str) != Some("object") {
                        v.errors.push(Issue::new(
                            format!("{path}.inputSchema.type"),
                            "inputSchema 的 type 必须为 \"object\"",
                        ));
                    }
                }
            }
            // outputSchema 可以是任意根类型（非 object 时 Hub 按 MCP 要求包装，spec/protocol.md 3.2），但必须是 JSON Schema 对象。
            if tool.output_schema.as_ref().is_some_and(|s| !s.is_object()) {
                v.errors.push(Issue::new(format!("{path}.outputSchema"), "outputSchema 必须是对象（JSON Schema）"));
            }
        }

        let mut seen = HashSet::new();
        for (i, res) in self.resources.iter().enumerate() {
            let path = format!("resources[{i}]");
            if !is_valid_name(&res.name) {
                v.errors.push(Issue::new(
                    format!("{path}.name"),
                    format!(
                        "资源名 \"{}\" 不合法，应满足 [a-zA-Z0-9_.-]{{1,64}}",
                        res.name
                    ),
                ));
            } else if !seen.insert(res.name.as_str()) {
                v.errors.push(Issue::new(
                    format!("{path}.name"),
                    format!("资源名 \"{}\" 重复", res.name),
                ));
            }
            if res.description.is_empty() {
                v.errors.push(Issue::new(
                    format!("{path}.description"),
                    "description 不能为空字符串",
                ));
            }
        }

        self.validate_launch(&mut v);
        self.validate_wake(&mut v);
        v
    }

    fn validate_wake(&self, v: &mut Validation) {
        for platform in WAKE_PLATFORMS {
            for (i, entry) in self.wake.platform(platform).iter().enumerate() {
                let path = format!("wake.{platform}[{i}]");
                match entry {
                    WakeEntry::Known(d) => validate_wake_descriptor(d, platform, &path, v),
                    WakeEntry::Other(value) => match value.get("kind").and_then(Value::as_str) {
                        None => v
                            .errors
                            .push(Issue::new(&path, "唤醒描述必须是带字符串 kind 字段的对象")),
                        Some(k) if KNOWN_WAKE_KINDS.contains(&k) => v.errors.push(Issue::new(
                            &path,
                            format!("唤醒描述 \"{k}\" 的字段类型错误（target 应为字符串，background 应为布尔）"),
                        )),
                        Some(k) => v
                            .warnings
                            .push(Issue::new(&path, format!("未知的唤醒方式 \"{k}\"，已忽略"))),
                    },
                }
            }
        }
        for key in self.wake.other.keys() {
            v.warnings.push(Issue::new(
                format!("wake.{key}"),
                format!("未知的平台 \"{key}\"，已忽略"),
            ));
        }
    }

    fn validate_launch(&self, v: &mut Validation) {
        let platforms: [(&str, &Vec<LaunchEntry>); 4] = [
            ("web", &self.launch.web),
            ("windows", &self.launch.windows),
            ("macos", &self.launch.macos),
            ("linux", &self.launch.linux),
        ];
        for (platform, entries) in platforms {
            for (i, entry) in entries.iter().enumerate() {
                let path = format!("launch.{platform}[{i}]");
                match entry {
                    LaunchEntry::Known(known) => {
                        if !known.platforms().contains(&platform) {
                            v.warnings.push(Issue::new(
                                &path,
                                format!(
                                    "唤醒方式 \"{}\" 不适用于平台 {platform}",
                                    known.type_name()
                                ),
                            ));
                        }
                        if let KnownLaunch::Url { href } = known
                            && !(href.starts_with("http://") || href.starts_with("https://"))
                        {
                            v.warnings.push(Issue::new(
                                &path,
                                format!("href \"{href}\" 不是 http(s) 地址"),
                            ));
                        }
                    }
                    LaunchEntry::Other(value) => match value.get("type").and_then(Value::as_str) {
                        None => v
                            .errors
                            .push(Issue::new(&path, "唤醒条目必须是带字符串 type 字段的对象")),
                        Some(t) if KNOWN_LAUNCH_TYPES.contains(&t) => v.errors.push(Issue::new(
                            &path,
                            format!("唤醒方式 \"{t}\" 缺少必需字段或字段类型错误"),
                        )),
                        Some(t) => v
                            .warnings
                            .push(Issue::new(&path, format!("未知的唤醒方式 \"{t}\"，已忽略"))),
                    },
                }
            }
        }
        for key in self.launch.other.keys() {
            v.warnings.push(Issue::new(
                format!("launch.{key}"),
                format!("未知的平台 \"{key}\"，已忽略"),
            ));
        }
    }

    /// `launch.web` 中第一个 `url` 条目的地址，用于未连接时的唤醒提示。
    pub fn web_url(&self) -> Option<&str> {
        self.launch.web.iter().find_map(|e| match e {
            LaunchEntry::Known(KnownLaunch::Url { href }) => Some(href.as_str()),
            _ => None,
        })
    }

    /// 某个平台的首选唤醒描述：`wake.<platform>` 中第一个已识别的条目。
    ///
    /// Web 平台未声明 `wake.web` 时，由 `launch.web` 的第一个 `url` 推导出 `web-url`（spec/manifest.md 第 2.2 节）。
    pub fn wake_descriptor(&self, platform: &str) -> Option<WakeDescriptor> {
        if let Some(d) = self.wake.descriptors(platform).next() {
            return Some(d.clone());
        }
        if platform == "web" && self.wake.web.is_empty() {
            return self.web_url().map(|href| WakeDescriptor {
                kind: WakeKind::WebUrl,
                target: Some(href.to_string()),
                background: false,
            });
        }
        None
    }

    pub fn tool(&self, name: &str) -> Option<&ToolInfo> {
        self.tools.iter().find(|t| t.name == name)
    }

    pub fn resource(&self, name: &str) -> Option<&ResourceInfo> {
        self.resources.iter().find(|r| r.name == name)
    }
}

fn validate_wake_descriptor(d: &WakeDescriptor, platform: &str, path: &str, v: &mut Validation) {
    let kind = d.kind.as_str();
    if !d.kind.platforms().contains(&platform) {
        v.warnings.push(Issue::new(
            path,
            format!("唤醒方式 \"{kind}\" 不适用于平台 {platform}"),
        ));
    }
    let target = d.target.as_deref().map(str::trim);
    if d.kind == WakeKind::None {
        if target.is_some() {
            v.warnings
                .push(Issue::new(path, "唤醒方式 \"none\" 不需要 target，已忽略"));
        }
        return;
    }
    let Some(target) = target.filter(|t| !t.is_empty()) else {
        v.errors.push(Issue::new(
            format!("{path}.target"),
            format!("唤醒方式 \"{kind}\" 缺少 target"),
        ));
        return;
    };
    let target_path = format!("{path}.target");
    match d.kind {
        WakeKind::WebUrl => {
            if !(target.starts_with("http://") || target.starts_with("https://")) {
                v.errors.push(Issue::new(
                    target_path,
                    format!("web-url 的 target \"{target}\" 必须是 http(s) 地址"),
                ));
            } else if target.contains('#') {
                v.warnings.push(Issue::new(
                    target_path,
                    "web-url 的 target 不应包含片段（#），Host 会附加 #app-mcp-wake=<token>",
                ));
            }
            if d.background {
                v.warnings
                    .push(Issue::new(path, "网页无法在后台唤醒，background 将被忽略"));
            }
        }
        WakeKind::Uri => {
            if !is_valid_uri_scheme(target) {
                v.errors.push(Issue::new(
                    target_path,
                    format!("uri 的 target 应为 scheme（如 shop-app），\"{target}\" 不合法"),
                ));
            } else if matches!(
                target.to_ascii_lowercase().as_str(),
                "http" | "https" | "file" | "javascript" | "data"
            ) {
                v.errors.push(Issue::new(
                    target_path,
                    format!("uri 的 target 不能是 \"{target}\"，网页请用 web-url"),
                ));
            }
        }
        WakeKind::AndroidIntent => {
            let valid = target
                .split_once('/')
                .is_some_and(|(pkg, class)| !pkg.is_empty() && !class.is_empty());
            if !valid {
                v.errors.push(Issue::new(
                    target_path,
                    format!(
                        "android-intent 的 target 应为 <包名>/<接收器类名>，实际为 \"{target}\""
                    ),
                ));
            }
        }
        WakeKind::Dbus => {
            if !target.contains('.') {
                v.warnings.push(Issue::new(
                    target_path,
                    format!("D-Bus 名称 \"{target}\" 通常应为反向域名形式"),
                ));
            }
        }
        WakeKind::Aumid | WakeKind::AppleEvent | WakeKind::None => {}
    }
}

/// 加载失败。
#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("读取清单 {path} 失败：{source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("解析清单{} 失败：{source}", display_path(.path))]
    Parse {
        path: Option<PathBuf>,
        #[source]
        source: serde_json::Error,
    },
    #[error("清单{} 校验失败：{}", display_path(.path), join_issues(.errors))]
    Invalid {
        path: Option<PathBuf>,
        errors: Vec<Issue>,
    },
}

fn display_path(path: &Option<PathBuf>) -> String {
    path.as_ref()
        .map(|p| format!(" {}", p.display()))
        .unwrap_or_default()
}

fn join_issues(issues: &[Issue]) -> String {
    issues
        .iter()
        .map(Issue::to_string)
        .collect::<Vec<_>>()
        .join("；")
}

/// 解析清单文本（只做结构解析）。
pub fn parse(text: &str) -> Result<Manifest, ManifestError> {
    serde_json::from_str(text).map_err(|source| ManifestError::Parse { path: None, source })
}

/// 解析并校验成功的清单。
#[derive(Clone, Debug, PartialEq)]
pub struct LoadedManifest {
    /// 来源文件；从文本加载时为 `None`。
    pub path: Option<PathBuf>,
    pub manifest: Manifest,
    pub warnings: Vec<Issue>,
}

/// 解析并校验清单文本。
pub fn load_str(text: &str) -> Result<LoadedManifest, ManifestError> {
    let manifest = parse(text)?;
    let validation = manifest.validate();
    if !validation.is_ok() {
        return Err(ManifestError::Invalid {
            path: None,
            errors: validation.errors,
        });
    }
    Ok(LoadedManifest {
        path: None,
        manifest,
        warnings: validation.warnings,
    })
}

/// 读取、解析并校验单个清单文件。
pub fn load_file(path: impl AsRef<Path>) -> Result<LoadedManifest, ManifestError> {
    let path = path.as_ref();
    let text = std::fs::read_to_string(path).map_err(|source| ManifestError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    load_str(&text)
        .map(|mut loaded| {
            loaded.path = Some(path.to_path_buf());
            loaded
        })
        .map_err(|e| match e {
            ManifestError::Parse { source, .. } => ManifestError::Parse {
                path: Some(path.to_path_buf()),
                source,
            },
            ManifestError::Invalid { errors, .. } => ManifestError::Invalid {
                path: Some(path.to_path_buf()),
                errors,
            },
            other => other,
        })
}

/// 读取目录下所有 `*.json`（不递归），按文件名排序后逐个加载。
///
/// 目录本身无法读取时返回 `Err`；单个文件的失败放在结果列表中，不影响其他文件。
pub fn load_dir(
    dir: impl AsRef<Path>,
) -> std::io::Result<Vec<Result<LoadedManifest, ManifestError>>> {
    let mut paths = Vec::new();
    for entry in std::fs::read_dir(dir.as_ref())? {
        let path = entry?.path();
        let is_json = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("json"));
        if is_json && path.is_file() {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths.into_iter().map(load_file).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn example() -> Value {
        json!({
            "manifestVersion": 1,
            "appId": "shop",
            "name": "示例商城",
            "version": "1.2.0",
            "description": "一个用于演示的购物商城",
            "overview": {
                "summary": "演示用购物商城，可管理待办、浏览商品、操作购物车并结算",
                "body": "## 能力范围\n- 待办的增删改查",
                "locale": "zh-CN"
            },
            "launch": {
                "web": [{ "type": "url", "href": "http://localhost:5173/" }],
                "windows": [{ "type": "uri", "scheme": "shop-app" },
                            { "type": "aumid", "id": "Company.Shop_xxx!App" },
                            { "type": "exe", "path": "%LOCALAPPDATA%\\Shop\\Shop.exe" }],
                "macos": [{ "type": "bundle", "id": "com.company.shop" }],
                "linux": [{ "type": "dbus", "name": "com.company.Shop" },
                          { "type": "desktop", "file": "com.company.Shop.desktop" }]
            },
            "wake": {
                "web": [{ "kind": "web-url", "target": "http://localhost:5173/" }],
                "windows": [{ "kind": "aumid", "target": "Company.Shop_xxx!App" },
                            { "kind": "uri", "target": "shop-app" }],
                "macos": [{ "kind": "uri", "target": "shop-app", "background": true }],
                "linux": [{ "kind": "dbus", "target": "com.company.Shop", "background": true }],
                "android": [{ "kind": "android-intent", "target": "com.company.shop/dev.appmcp.WakeReceiver", "background": true }],
                "ios": [{ "kind": "uri", "target": "shop-app" }]
            },
            "tools": [{
                "name": "orders.search",
                "title": "搜索订单",
                "description": "按关键词搜索历史订单",
                "inputSchema": { "type": "object", "properties": { "keyword": { "type": "string" } } },
                "risk": "read",
                "activation": "headless"
            }],
            "resources": [{ "name": "cart.state", "description": "当前购物车内容与总价" }]
        })
    }

    fn with(mut base: Value, pointer: &str, value: Value) -> Manifest {
        *base.pointer_mut(pointer).expect("pointer exists") = value;
        serde_json::from_value(base).expect("parses")
    }

    #[test]
    fn tool_name_with_app_id_prefix_warns() {
        let m = with(example(), "/tools/0/name", json!("shop.info"));
        let v = m.validate();
        assert!(v.is_ok(), "协议上仍合法：{:?}", v.errors);
        assert_eq!(v.warnings.len(), 1, "{:?}", v.warnings);
        assert_eq!(v.warnings[0].path, "tools[0].name");
        assert!(v.warnings[0].message.contains("\"info\""));
    }

    #[test]
    fn parses_spec_example() {
        let m = parse(&example().to_string()).unwrap();
        assert_eq!(m.app_id, "shop");
        assert_eq!(m.tools.len(), 1);
        assert_eq!(m.tools[0].risk, app_mcp_protocol::Risk::Read);
        assert_eq!(m.web_url(), Some("http://localhost:5173/"));
        assert_eq!(m.launch.windows.len(), 3);
        assert!(matches!(
            m.launch.linux[1],
            LaunchEntry::Known(KnownLaunch::Desktop { .. })
        ));
        let v = m.validate();
        assert!(v.is_ok(), "{:?}", v.errors);
        assert!(v.warnings.is_empty(), "{:?}", v.warnings);
    }

    #[test]
    fn overview_parsing_and_validation() {
        let m = parse(&example().to_string()).unwrap();
        let ov = m.overview.as_ref().unwrap();
        assert!(ov.summary.starts_with("演示用购物商城"));
        assert_eq!(ov.locale.as_deref(), Some("zh-CN"));
        assert_eq!(
            serde_json::to_value(&m).unwrap()["overview"]["body"],
            json!("## 能力范围\n- 待办的增删改查")
        );

        // summary 为空 → 错误
        let v = with(example(), "/overview/summary", json!("  ")).validate();
        assert_eq!(v.errors.len(), 1);
        assert_eq!(v.errors[0].path, "overview.summary");

        // 超长 → 警告（按字符计数，不是字节）
        let v = with(example(), "/overview/summary", json!("商".repeat(101))).validate();
        assert!(v.is_ok());
        assert_eq!(v.warnings.len(), 1);
        assert!(
            with(example(), "/overview/summary", json!("商".repeat(100)))
                .validate()
                .warnings
                .is_empty()
        );
        let v = with(example(), "/overview/body", json!("x".repeat(2001))).validate();
        assert!(v.is_ok());
        assert_eq!(v.warnings[0].path, "overview.body");

        // 缺 summary → 结构错误
        let mut base = example();
        base["overview"] = json!({ "body": "x" });
        assert!(parse(&base.to_string()).is_err());

        // 可省略
        let mut base = example();
        base.as_object_mut().unwrap().remove("overview");
        let m: Manifest = serde_json::from_value(base).unwrap();
        assert!(m.overview.is_none());
        assert!(m.validate().is_ok());
    }

    #[test]
    fn roundtrip_keeps_fields() {
        let m = parse(&example().to_string()).unwrap();
        let back: Manifest = serde_json::from_value(serde_json::to_value(&m).unwrap()).unwrap();
        assert_eq!(back, m);
        assert_eq!(serde_json::to_value(&m).unwrap()["appId"], json!("shop"));
    }

    #[test]
    fn minimal_manifest() {
        let m = parse(r#"{"manifestVersion":1,"appId":"x","name":"X"}"#).unwrap();
        assert!(m.validate().is_ok());
        assert!(m.launch.is_empty());
        assert_eq!(m.web_url(), None);
    }

    #[test]
    fn unknown_launch_type_is_preserved_with_warning() {
        let entry = json!({ "type": "flatpak", "ref": "com.company.Shop", "extra": [1, 2] });
        let m = with(example(), "/launch/linux/0", entry.clone());
        assert_eq!(m.launch.linux[0], LaunchEntry::Other(entry.clone()));
        let v = m.validate();
        assert!(v.is_ok());
        assert_eq!(v.warnings.len(), 1);
        assert!(v.warnings[0].path.starts_with("launch.linux[0]"));
        // 原样透传
        let out = serde_json::to_value(&m).unwrap();
        assert_eq!(out["launch"]["linux"][0], entry);
    }

    #[test]
    fn unknown_platform_is_preserved_with_warning() {
        let mut base = example();
        base["launch"]["android"] = json!([{ "type": "intent", "action": "x" }]);
        let m: Manifest = serde_json::from_value(base).unwrap();
        assert!(m.launch.other.contains_key("android"));
        let v = m.validate();
        assert!(v.is_ok());
        assert_eq!(v.warnings.len(), 1);
        assert_eq!(
            serde_json::to_value(&m).unwrap()["launch"]["android"][0]["type"],
            json!("intent")
        );
    }

    #[test]
    fn known_launch_type_missing_field_is_error() {
        let m = with(example(), "/launch/web/0", json!({ "type": "url" }));
        assert!(!m.validate().is_ok());
        let m = with(example(), "/launch/web/0", json!({ "href": "http://x" }));
        assert!(!m.validate().is_ok());
    }

    #[test]
    fn launch_platform_mismatch_warns() {
        let m = with(
            example(),
            "/launch/macos/0",
            json!({ "type": "aumid", "id": "x" }),
        );
        let v = m.validate();
        assert!(v.is_ok());
        assert_eq!(v.warnings.len(), 1);
    }

    #[test]
    fn wake_parses_and_roundtrips() {
        let m = parse(&example().to_string()).unwrap();
        assert_eq!(m.wake.windows.len(), 2);
        let d = m.wake.descriptors("android").next().unwrap();
        assert_eq!(d.kind, WakeKind::AndroidIntent);
        assert!(d.background);
        let d = m.wake.descriptors("windows").next().unwrap();
        assert_eq!((d.kind, d.background), (WakeKind::Aumid, false));
        assert_eq!(
            m.wake_descriptor("web").unwrap().target.as_deref(),
            Some("http://localhost:5173/")
        );
        assert!(m.wake_descriptor("unknown").is_none());
        // background: false 序列化时省略
        let out = serde_json::to_value(&m).unwrap();
        assert_eq!(
            out["wake"]["windows"][1],
            json!({ "kind": "uri", "target": "shop-app" })
        );
        assert_eq!(out["wake"]["macos"][0]["background"], json!(true));
        let back: Manifest = serde_json::from_value(out).unwrap();
        assert_eq!(back, m);
        // 缺省
        let m = parse(r#"{"manifestVersion":1,"appId":"x","name":"X"}"#).unwrap();
        assert!(m.wake.is_empty());
        assert!(
            !serde_json::to_value(&m)
                .unwrap()
                .as_object()
                .unwrap()
                .contains_key("wake")
        );
    }

    #[test]
    fn web_wake_falls_back_to_launch_url() {
        let mut base = example();
        base.as_object_mut().unwrap().remove("wake");
        let m: Manifest = serde_json::from_value(base).unwrap();
        let d = m.wake_descriptor("web").unwrap();
        assert_eq!(d.kind, WakeKind::WebUrl);
        assert_eq!(d.target.as_deref(), Some("http://localhost:5173/"));
        assert!(m.wake_descriptor("windows").is_none());
        // 显式声明 none 时不回退
        let m = with(example(), "/wake/web", json!([{ "kind": "none" }]));
        assert_eq!(m.wake_descriptor("web").unwrap().kind, WakeKind::None);
        assert!(m.validate().is_ok());
    }

    #[test]
    fn wake_target_rules() {
        let err = |pointer: &str, value: Value| {
            let v = with(example(), pointer, value).validate();
            assert!(!v.is_ok(), "{pointer}");
            v.errors[0].path.clone()
        };
        assert_eq!(
            err("/wake/web/0", json!({ "kind": "web-url" })),
            "wake.web[0].target"
        );
        assert_eq!(
            err("/wake/web/0", json!({ "kind": "web-url", "target": " " })),
            "wake.web[0].target"
        );
        err(
            "/wake/web/0",
            json!({ "kind": "web-url", "target": "shop-app://x" }),
        );
        err(
            "/wake/windows/1",
            json!({ "kind": "uri", "target": "shop app" }),
        );
        err(
            "/wake/windows/1",
            json!({ "kind": "uri", "target": "https" }),
        );
        err(
            "/wake/android/0",
            json!({ "kind": "android-intent", "target": "com.company.shop" }),
        );
        // 字段类型错误 → 保留为 Other，报错
        let m = with(
            example(),
            "/wake/macos/0",
            json!({ "kind": "uri", "target": 1 }),
        );
        assert!(matches!(m.wake.macos[0], WakeEntry::Other(_)));
        assert!(!m.validate().is_ok());
        err(
            "/wake/macos/0",
            json!({ "kind": "uri", "target": "x", "background": "yes" }),
        );
        err("/wake/macos/0", json!({ "target": "x" }));
        err("/wake/macos/0", json!("shop-app"));
    }

    #[test]
    fn wake_warnings() {
        let warn = |pointer: &str, value: Value| {
            let v = with(example(), pointer, value).validate();
            assert!(v.is_ok(), "{pointer}: {:?}", v.errors);
            assert_eq!(v.warnings.len(), 1, "{pointer}: {:?}", v.warnings);
            v.warnings[0].message.clone()
        };
        // 平台不匹配
        assert!(
            warn("/wake/macos/0", json!({ "kind": "aumid", "target": "x" })).contains("不适用")
        );
        // 网页不能后台唤醒
        assert!(
            warn(
                "/wake/web/0",
                json!({ "kind": "web-url", "target": "https://a/", "background": true })
            )
            .contains("background")
        );
        warn(
            "/wake/web/0",
            json!({ "kind": "web-url", "target": "https://a/#x" }),
        );
        warn("/wake/linux/0", json!({ "kind": "dbus", "target": "shop" }));
        warn("/wake/ios/0", json!({ "kind": "none", "target": "x" }));
        // 未知 kind 原样保留
        let entry = json!({ "kind": "flatpak", "ref": "com.company.Shop" });
        let m = with(example(), "/wake/linux/0", entry.clone());
        assert_eq!(m.wake.linux[0], WakeEntry::Other(entry.clone()));
        assert_eq!(serde_json::to_value(&m).unwrap()["wake"]["linux"][0], entry);
        assert_eq!(m.validate().warnings.len(), 1);
        assert_eq!(m.wake.descriptors("linux").count(), 0);
        // 未知平台原样保留
        let mut base = example();
        base["wake"]["harmony"] = json!([{ "kind": "uri", "target": "shop" }]);
        let m: Manifest = serde_json::from_value(base).unwrap();
        assert!(m.wake.other.contains_key("harmony"));
        let v = m.validate();
        assert!(v.is_ok());
        assert_eq!(v.warnings[0].path, "wake.harmony");
    }

    #[test]
    fn wrong_version() {
        let v = with(example(), "/manifestVersion", json!(2)).validate();
        assert_eq!(v.errors.len(), 1);
        assert_eq!(v.errors[0].path, "manifestVersion");
    }

    #[test]
    fn bad_app_id() {
        assert!(!with(example(), "/appId", json!("Shop")).validate().is_ok());
        assert!(
            !with(example(), "/appId", json!("my.app"))
                .validate()
                .is_ok()
        );
        assert!(!with(example(), "/appId", json!("")).validate().is_ok());
    }

    #[test]
    fn reserved_app_ids() {
        for id in RESERVED_APP_IDS {
            let v = with(example(), "/appId", json!(id)).validate();
            assert_eq!(v.errors.len(), 1, "{id}");
            assert!(v.errors[0].message.contains("保留"));
        }
        assert!(with(example(), "/appId", json!("apps2")).validate().is_ok());
    }

    #[test]
    fn bad_tool_name_and_duplicates() {
        assert!(
            !with(example(), "/tools/0/name", json!("has space"))
                .validate()
                .is_ok()
        );
        let mut base = example();
        let tool = base["tools"][0].clone();
        base["tools"].as_array_mut().unwrap().push(tool);
        let m: Manifest = serde_json::from_value(base).unwrap();
        let v = m.validate();
        assert_eq!(v.errors.len(), 1);
        assert!(v.errors[0].message.contains("重复"));
    }

    /// `resources[].realtime`（spec/manifest.md、spec/lifecycle.md 第 13 节 B3）：缺省 false，可声明 true。
    #[test]
    fn resource_realtime_flag() {
        let m: Manifest = serde_json::from_value(example()).unwrap();
        assert!(!m.resource("cart.state").unwrap().realtime);
        let mut base = example();
        base["resources"][0]["realtime"] = json!(true);
        let m: Manifest = serde_json::from_value(base).unwrap();
        assert!(m.validate().is_ok());
        assert!(m.resource("cart.state").unwrap().realtime);
    }

    #[test]
    fn annotations_and_output_schema() {
        let mut base = example();
        base["tools"][0]["annotations"] = json!({"readOnlyHint": true, "openWorldHint": false});
        base["tools"][0]["outputSchema"] = json!({"type": "array", "items": {"type": "string"}});
        base["resources"][0]["annotations"] = json!({"audience": ["user"], "priority": 0.8});
        let m: Manifest = serde_json::from_value(base.clone()).unwrap();
        assert!(m.validate().is_ok(), "{:?}", m.validate().errors);
        let t = &m.tools[0];
        assert_eq!(t.annotations.as_ref().unwrap().open_world_hint, Some(false));
        assert_eq!(t.output_schema.as_ref().unwrap()["type"], "array");
        assert_eq!(m.resources[0].annotations.as_ref().unwrap().priority, Some(0.8));
        // 往返保持字段
        let back: Manifest = serde_json::from_value(serde_json::to_value(&m).unwrap()).unwrap();
        assert_eq!(back, m);
        // outputSchema 不是对象 → 错误；注解字段类型不对 → 解析失败
        base["tools"][0]["outputSchema"] = json!("array");
        let m: Manifest = serde_json::from_value(base.clone()).unwrap();
        assert!(m.validate().errors.iter().any(|e| e.path == "tools[0].outputSchema"));
        base["tools"][0]["annotations"] = json!({"readOnlyHint": "yes"});
        assert!(serde_json::from_value::<Manifest>(base).is_err());
    }

    #[test]
    fn bad_resource_name_and_duplicates() {
        assert!(
            !with(example(), "/resources/0/name", json!(""))
                .validate()
                .is_ok()
        );
        let mut base = example();
        let res = base["resources"][0].clone();
        base["resources"].as_array_mut().unwrap().push(res);
        let m: Manifest = serde_json::from_value(base).unwrap();
        assert!(!m.validate().is_ok());
    }

    #[test]
    fn input_schema_must_be_object_type() {
        assert!(
            !with(
                example(),
                "/tools/0/inputSchema",
                json!({ "type": "array" })
            )
            .validate()
            .is_ok()
        );
        assert!(
            !with(example(), "/tools/0/inputSchema", json!({}))
                .validate()
                .is_ok()
        );
        assert!(
            !with(example(), "/tools/0/inputSchema", json!("object"))
                .validate()
                .is_ok()
        );
    }

    #[test]
    fn empty_descriptions() {
        assert!(
            !with(example(), "/description", json!(""))
                .validate()
                .is_ok()
        );
        assert!(
            !with(example(), "/tools/0/description", json!(""))
                .validate()
                .is_ok()
        );
        assert!(
            !with(example(), "/resources/0/description", json!(""))
                .validate()
                .is_ok()
        );
    }

    #[test]
    fn structural_errors() {
        assert!(matches!(
            parse("not json"),
            Err(ManifestError::Parse { .. })
        ));
        assert!(parse(r#"{"manifestVersion":1,"name":"x"}"#).is_err()); // 缺 appId
        assert!(parse(r#"{"manifestVersion":"1","appId":"x","name":"x"}"#).is_err());
        assert!(matches!(
            load_str(r#"{"manifestVersion":1,"appId":"apps","name":"x"}"#),
            Err(ManifestError::Invalid { .. })
        ));
    }

    #[test]
    fn load_file_and_dir() {
        let dir =
            std::env::temp_dir().join(format!("app-mcp-manifest-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.json"), example().to_string()).unwrap();
        std::fs::write(dir.join("b.json"), "{ broken").unwrap();
        std::fs::write(
            dir.join("c.json"),
            r#"{"manifestVersion":2,"appId":"c","name":"C"}"#,
        )
        .unwrap();
        std::fs::write(dir.join("notes.txt"), "ignored").unwrap();

        let loaded = load_file(dir.join("a.json")).unwrap();
        assert_eq!(loaded.manifest.app_id, "shop");
        assert_eq!(loaded.path.as_deref(), Some(dir.join("a.json").as_path()));
        assert!(matches!(
            load_file(dir.join("missing.json")),
            Err(ManifestError::Io { .. })
        ));

        let all = load_dir(&dir).unwrap();
        assert_eq!(all.len(), 3);
        assert!(all[0].is_ok());
        assert!(matches!(
            &all[1],
            Err(ManifestError::Parse { path: Some(_), .. })
        ));
        match &all[2] {
            Err(e @ ManifestError::Invalid { path: Some(_), .. }) => {
                assert!(e.to_string().contains("manifestVersion"))
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(load_dir(dir.join("nope")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
