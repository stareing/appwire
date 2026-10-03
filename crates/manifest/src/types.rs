//! 清单数据结构：页面、launch、wake 描述。

use super::*;

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
    /// 页面目录（第 4c 项，spec/manifest.md 2.3）：各页面的说明与页面内工具；Hub 渐进披露并在调用时先导航再派发。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pages: Vec<Page>,
    /// 可发出的事件（第 16 项 N3，spec/manifest.md 2.4）；Agent 据此订阅。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<EventInfo>,
}

/// 页面条目（spec/manifest.md 2.3）。页面内工具不作为静态工具列出，只进入页面目录。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    /// 页面名 `[a-zA-Z0-9_.-]{1,64}`，清单内唯一；即 `app/navigate` 的 `page`。
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// App 内路由（如 `/orders/:id`），供 App 与构建工具使用；Host 不解析。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
    /// 导航参数的 JSON Schema（`type` 为 `"object"`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
    /// 页面内的工具（结构同协议 `ToolInfo`）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolInfo>,
    /// 能否由 Agent 导航到该页面；缺省 `true`，`true` 时不序列化。
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub navigable: bool,
    /// 导航到该页面需要的激活方式。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activation: Option<Activation>,
}

fn default_true() -> bool {
    true
}

fn is_true(b: &bool) -> bool {
    *b
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
    pub(crate) fn platforms(&self) -> &'static [&'static str] {
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

pub(crate) const KNOWN_LAUNCH_TYPES: &[&str] = &["url", "uri", "aumid", "exe", "bundle", "dbus", "desktop"];

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

pub(crate) const KNOWN_WAKE_KINDS: &[&str] = &[
    "uri",
    "aumid",
    "apple-event",
    "dbus",
    "android-intent",
    "web-url",
    "none",
];

pub(crate) fn is_valid_uri_scheme(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

impl Manifest {
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

    pub fn page(&self, name: &str) -> Option<&Page> {
        self.pages.iter().find(|p| p.name == name)
    }

    /// 在某个页面（`pages[].tools`）中声明的工具 → `(页面, 工具)`。
    pub fn page_tool(&self, tool: &str) -> Option<(&Page, &ToolInfo)> {
        self.pages.iter().find_map(|p| p.tools.iter().find(|t| t.name == tool).map(|t| (p, t)))
    }
}
