//! 按名寻址（spec/naming.md）：地址 `appmcp://<appId>[/<instance>]`、各平台名字映射与名字服务错误码。
//!
//! 只做纯数据转换（解析、映射、反向映射），不做 I/O；名字服务的登记（App 侧）与拨号（Hub 侧）分别在
//! `app-mcp-native` 与 `app-mcp-hub` 中实现。

use std::fmt;

use crate::is_valid_app_id;

/// 地址的 scheme 前缀（spec/naming.md 2.1，只接受小写）。
pub const ADDRESS_SCHEME: &str = "appmcp://";

/// 保留的实例名：指默认名字本身，不能作为登记实例名（2.2）。
pub const RESERVED_INSTANCE: &str = "default";

/// 登记实例名的最大长度（2.1 `instance`）。
pub const MAX_INSTANCE_LEN: usize = 32;

/// 一个 App（或其一个登记实例）的地址（spec/naming.md 第 2 节）。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Address {
    pub app_id: String,
    /// 登记实例名；`None` = 默认名字（唯一可被系统激活的名字）。
    pub instance: Option<String>,
}

/// 地址不符合规范形式（错误码 [`codes::INVALID_ADDRESS`]）。
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("地址无效（应为 appmcp://<appId>[/<instance>]）：{0}")]
pub struct InvalidAddress(pub String);

/// 登记实例名是否合法：`[a-z][a-z0-9-]{0,31}`，且不是保留名 `default`。
pub fn is_valid_instance(name: &str) -> bool {
    let b = name.as_bytes();
    !b.is_empty()
        && b.len() <= MAX_INSTANCE_LEN
        && b[0].is_ascii_lowercase()
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
        && name != RESERVED_INSTANCE
}

impl Address {
    /// 校验后构造；`app_id` 只检查格式（保留名由调用方按 spec/naming.md 2.2 另行拒绝）。
    pub fn new(app_id: &str, instance: Option<&str>) -> Result<Self, InvalidAddress> {
        if !is_valid_app_id(app_id) {
            return Err(InvalidAddress(format!("appId「{app_id}」应满足 [a-z][a-z0-9-]{{0,62}}")));
        }
        if let Some(i) = instance
            && !is_valid_instance(i)
        {
            return Err(InvalidAddress(format!("实例名「{i}」应满足 [a-z][a-z0-9-]{{0,31}} 且不是 default")));
        }
        Ok(Self { app_id: app_id.to_owned(), instance: instance.map(str::to_owned) })
    }

    /// 解析规范形式；任何其他形式（大写、端口、查询串、尾部 `/` 等）显式失败，不做规范化猜测。
    pub fn parse(text: &str) -> Result<Self, InvalidAddress> {
        let rest = text
            .strip_prefix(ADDRESS_SCHEME)
            .ok_or_else(|| InvalidAddress(format!("{text:?} 不以 {ADDRESS_SCHEME} 开头")))?;
        let (app_id, instance) = match rest.split_once('/') {
            Some((a, i)) => (a, Some(i)),
            None => (rest, None),
        };
        Self::new(app_id, instance).map_err(|e| InvalidAddress(format!("{text:?}：{}", e.0)))
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{ADDRESS_SCHEME}{}", self.app_id)?;
        if let Some(i) = &self.instance {
            write!(f, "/{i}")?;
        }
        Ok(())
    }
}

/// Linux D-Bus 会话总线上的名字映射（spec/naming.md 4.1）。
pub mod dbus {
    use super::{Address, is_valid_instance};
    use crate::is_valid_app_id;

    /// 总线名前缀（含末尾的 `.`）。
    pub const BUS_NAME_PREFIX: &str = "dev.appmcp.App.";
    /// 接口名（带主版本号）。
    pub const INTERFACE: &str = "dev.appmcp.App1";
    /// 默认名字的对象路径。
    pub const OBJECT_PATH: &str = "/dev/appmcp/App";
    /// `Open` 方法名。
    pub const OPEN_METHOD: &str = "Open";
    /// 激活文件中 `Exec` 追加的参数：App 据此知道自己由名字服务激活（spec/naming.md 4.1）。
    pub const ACTIVATION_ARG: &str = "--app-mcp-activation";

    /// `-` 写为 `_`（`appId` / `instance` 都不含 `_`，映射可逆）。
    fn escape(s: &str) -> String {
        s.replace('-', "_")
    }

    fn unescape(s: &str) -> String {
        s.replace('_', "-")
    }

    /// 地址 → 总线名：`dev.appmcp.App.<id>`、`dev.appmcp.App.<id>.<inst>`。
    pub fn bus_name(address: &Address) -> String {
        match &address.instance {
            Some(i) => format!("{BUS_NAME_PREFIX}{}.{}", escape(&address.app_id), escape(i)),
            None => format!("{BUS_NAME_PREFIX}{}", escape(&address.app_id)),
        }
    }

    /// 地址 → 对象路径：`/dev/appmcp/App`、`/dev/appmcp/App/<inst>`。
    pub fn object_path(address: &Address) -> String {
        match &address.instance {
            Some(i) => format!("{OBJECT_PATH}/{}", escape(i)),
            None => OBJECT_PATH.to_owned(),
        }
    }

    /// 总线名 → 地址；不是本规范的名字（前缀不符、元素不合法）时为 `None`。
    pub fn parse_bus_name(name: &str) -> Option<Address> {
        let rest = name.strip_prefix(BUS_NAME_PREFIX)?;
        let (id, inst) = match rest.split_once('.') {
            Some((a, i)) => (a, Some(i)),
            None => (rest, None),
        };
        // 原文中出现 `-` 不是规范写法（映射写出的名字只含 `_`）：拒绝，保证一一对应。
        if id.contains('-') || inst.is_some_and(|i| i.contains('-') || i.contains('.')) {
            return None;
        }
        let app_id = unescape(id);
        let instance = inst.map(unescape);
        if !is_valid_app_id(&app_id) || instance.as_deref().is_some_and(|i| !is_valid_instance(i)) {
            return None;
        }
        Some(Address { app_id, instance })
    }

    /// 会话服务激活文件的文件名（与 `Name=` 一致）：`dev.appmcp.App.<id>.service`。
    pub fn service_file_name(app_id: &str) -> String {
        format!("{BUS_NAME_PREFIX}{}.service", escape(app_id))
    }

    /// 激活文件内容（spec/naming.md 4.1）：`Exec` 为程序路径加 [`ACTIVATION_ARG`]。
    ///
    /// @security `Exec` 中的程序路径按 D-Bus 激活文件的引号规则写出（双引号包裹，`"` 与 `\` 转义），
    /// 不经 shell 解释；调用方负责传入绝对路径。
    pub fn service_file(app_id: &str, exec: &str) -> String {
        let quoted = exec.replace('\\', "\\\\").replace('"', "\\\"");
        format!(
            "[D-BUS Service]\nName={BUS_NAME_PREFIX}{}\nExec=\"{quoted}\" {ACTIVATION_ARG}\n",
            escape(app_id)
        )
    }
}

/// Windows：每 App 每用户命名管道（spec/naming.md 4.3）。
pub mod pipe {
    use super::Address;

    /// 管道完整名前缀（含 `\\.\pipe\`）；与 Host 自己的管道 `\\.\pipe\app-mcp-<SID>` 不同。
    pub const PIPE_PREFIX: &str = r"\\.\pipe\appmcp-";
    /// 激活参数：与 D-Bus 激活文件的 `Exec` 相同（`exec` / `aumid` 激活时追加），SDK 据此知道自己由名字服务激活。
    pub const ACTIVATION_ARG: &str = super::dbus::ACTIVATION_ARG;
    /// App 拒绝一条通道时写给 Hub 的一行（`<CODE>：<说明>\n`）的最大字节数；之后 App 断开。
    pub const MAX_REFUSAL_LINE: usize = 512;

    /// 地址 → 管道完整名：`\\.\pipe\appmcp-<SID>-<appId>`、`\\.\pipe\appmcp-<SID>-<appId>.<instance>`。
    ///
    /// @input `sid` 为当前用户 SID 字符串（`S-1-5-21-…`）。实例分隔符为 `.`（appId / instance 都可含 `-`，用 `-` 有歧义）。
    pub fn pipe_name(sid: &str, address: &Address) -> String {
        match &address.instance {
            Some(i) => format!("{PIPE_PREFIX}{sid}-{}.{i}", address.app_id),
            None => format!("{PIPE_PREFIX}{sid}-{}", address.app_id),
        }
    }

    /// App 拒绝通道时写出的一行（不含换行）：`<CODE>：<说明>`，与 Android Binder 回复的写法相同（spec/naming.md 4.2）。
    pub fn refusal_line(code: &str, detail: &str) -> String {
        let mut line = format!("{code}：{}", detail.replace(['\r', '\n'], " "));
        if line.len() >= MAX_REFUSAL_LINE {
            let mut end = MAX_REFUSAL_LINE - 1;
            while !line.is_char_boundary(end) {
                end -= 1;
            }
            line.truncate(end);
        }
        line
    }

    /// 解析 App 的拒绝行：返回（错误码，说明）；码不是本规范的码时按 `ACTIVATION_DENIED`。
    pub fn parse_refusal(line: &str) -> (&'static str, String) {
        let line = line.trim_end_matches(['\r', '\n']);
        let (code, detail) = match line.split_once('：').or_else(|| line.split_once(':')) {
            Some((c, d)) => (c.trim(), d.trim()),
            None => (line.trim(), ""),
        };
        let code = super::codes::lookup(code).unwrap_or(super::codes::ACTIVATION_DENIED);
        (code, detail.to_owned())
    }

    /// 两个 Windows 可执行文件路径是否指同一文件（纯字符串比较）：去掉 `\\?\` 前缀、`/` 视为 `\`、不区分大小写。
    ///
    /// @why `std::fs::canonicalize` 在 Windows 上返回 `\\?\C:\…`，而 `QueryFullProcessImageNameW` 返回 `C:\…`。
    pub fn same_executable(a: &str, b: &str) -> bool {
        normalize_executable(a) == normalize_executable(b)
    }

    /// [`same_executable`] 的比较形式（小写）。
    pub fn normalize_executable(path: &str) -> String {
        strip_verbatim(&path.replace('/', "\\")).to_lowercase()
    }

    /// 去掉 Windows 扩展长度前缀：`\\?\C:\…` → `C:\…`、`\\?\UNC\srv\…` → `\\srv\…`（登记文件按此写出）。
    pub fn strip_verbatim(path: &str) -> String {
        match path.strip_prefix(r"\\?\UNC\") {
            Some(rest) => format!(r"\\{rest}"),
            None => path.strip_prefix(r"\\?\").unwrap_or(path).to_owned(),
        }
    }
}

/// App 登记文件（spec/naming.md 5.3）：桌面平台共用的 JSON，由安装程序 / `app-mcp-host app install` / SDK 自报写入，
/// Hub 只读（Windows 上是发现的唯一来源，4.3）。
pub mod registration {
    use serde::{Deserialize, Serialize};

    /// 当前格式版本。
    pub const REGISTRATION_VERSION: u32 = 1;

    /// 激活方式（`activation.kind`）。
    pub mod kinds {
        /// Linux D-Bus 服务激活（`target` = 总线名）。
        pub const DBUS: &str = "dbus";
        /// Windows 未打包 App：Hub 直接运行 `target`（缺省为 `executable`）并追加 `--app-mcp-activation`（4.3，2026-10-02 加入）。
        pub const EXEC: &str = "exec";
        /// 协议激活（`target` = URI scheme），沿用 WakeDescriptor `uri`。
        pub const URI: &str = "uri";
        /// Windows 打包 App：`IApplicationActivationManager::ActivateApplication`（`target` = AUMID，参数 `--app-mcp-activation`）。
        pub const AUMID: &str = "aumid";
        /// 不可激活：只在 App 运行时可拨号。
        pub const NONE: &str = "none";
    }

    /// 登记文件内容。路径字段为字符串（JSON 中的原文）。
    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct Registration {
        pub registration_version: u32,
        pub app_id: String,
        #[serde(default)]
        pub name: String,
        /// `install` | `manual` | `self-report`。
        pub source: String,
        /// 静态清单的绝对路径。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub manifest: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub manifest_sha256: Option<String>,
        /// 用于判断"已不存在"与核对通道对端（4.3：管道服务端进程映像）。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub executable: Option<String>,
        pub activation: Activation,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub struct Activation {
        /// [`kinds`] 之一；其他值按不可激活处理。
        pub kind: String,
        #[serde(default)]
        pub target: String,
    }

    /// 登记文件名：`<appId>.json`。
    pub fn file_name(app_id: &str) -> String {
        format!("{app_id}.json")
    }

    /// 登记目录：`<数据目录>/app-mcp/apps`（Linux `$XDG_DATA_HOME`，Windows `%LOCALAPPDATA%`，5.3）。
    pub fn apps_dir(data_root: &std::path::Path) -> std::path::PathBuf {
        data_root.join("app-mcp").join("apps")
    }

    /// 解析并校验登记文件（5.3）：版本为 [`REGISTRATION_VERSION`]、`appId` 合法且与文件名中的 `expected_app_id` 一致。
    ///
    /// @error 返回中文说明（调用方记录后忽略该文件）。保留名由调用方另行拒绝（依赖清单 crate）。
    pub fn parse(text: &str, expected_app_id: &str) -> Result<Registration, String> {
        let reg: Registration = serde_json::from_str(text).map_err(|e| format!("登记文件不是有效的 JSON：{e}"))?;
        if reg.registration_version != REGISTRATION_VERSION {
            return Err(format!(
                "不支持的 registrationVersion {}（本实现只支持 {REGISTRATION_VERSION}）",
                reg.registration_version
            ));
        }
        if !crate::is_valid_app_id(&reg.app_id) {
            return Err(format!("appId「{}」不合法", reg.app_id));
        }
        if reg.app_id != expected_app_id {
            return Err(format!("内容中的 appId「{}」与文件名「{expected_app_id}.json」不一致", reg.app_id));
        }
        Ok(reg)
    }
}

/// 名字服务相关的错误码（spec/naming.md 第 12 节）。
///
/// @compat 这些码尚未并入 [`crate::ConnectionErrorCode`]（spec/protocol.md 10.1 的表与各语言 SDK 的枚举共用），
/// 目前只出现在工具错误的 `data.code`、Hub `last_error` 与日志中；并入时字符串保持不变。
pub mod codes {
    pub const INVALID_ADDRESS: &str = "INVALID_ADDRESS";
    pub const NAME_NOT_FOUND: &str = "NAME_NOT_FOUND";
    pub const ACTIVATION_DENIED: &str = "ACTIVATION_DENIED";
    pub const ACTIVATION_TIMEOUT: &str = "ACTIVATION_TIMEOUT";
    pub const BIND_PERMISSION_DENIED: &str = "BIND_PERMISSION_DENIED";
    pub const PEER_IDENTITY_MISMATCH: &str = "PEER_IDENTITY_MISMATCH";
    pub const PEER_DIED: &str = "PEER_DIED";
    pub const CHANNEL_LIMIT: &str = "CHANNEL_LIMIT";
    /// App 拒绝了拨号的 Hub（spec/naming.md 10.2，Android `open()` 内的调用方校验）。
    pub const HUB_NOT_TRUSTED: &str = "HUB_NOT_TRUSTED";
    /// 目标已安装、组件存在，但系统拒绝绑定 / 激活（Android `bindService` 返回 false 或 `SecurityException`：关联启动 /
    /// 自启动管控、OEM 拦截）；需用户在系统设置中放行（spec/naming.md 第 12 节）。工具调用层为 `USER_ACTION_REQUIRED`。
    pub const ACTIVATION_BLOCKED: &str = "ACTIVATION_BLOCKED";
    /// 独立 Hub App 的原生库未包含 MCP 出口（cargo feature `mcp-server`），无法为 Agent 提供 MCP（spec/naming.md 第 12 节）。
    pub const HUB_UNSUPPORTED: &str = "HUB_UNSUPPORTED";

    /// 以上全部错误码（宿主语言回传错误码字符串时据此还原为本模块的常量）。
    pub const ALL: &[&str] = &[
        INVALID_ADDRESS,
        NAME_NOT_FOUND,
        ACTIVATION_DENIED,
        ACTIVATION_TIMEOUT,
        BIND_PERMISSION_DENIED,
        PEER_IDENTITY_MISMATCH,
        PEER_DIED,
        CHANNEL_LIMIT,
        HUB_NOT_TRUSTED,
        ACTIVATION_BLOCKED,
        HUB_UNSUPPORTED,
    ];

    /// 错误码字符串 → 本模块的常量；不是本规范的码时为 `None`。
    pub fn lookup(code: &str) -> Option<&'static str> {
        ALL.iter().copied().find(|c| *c == code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_canonical_forms() {
        let a = Address::parse("appmcp://my-shop").unwrap();
        assert_eq!(a, Address { app_id: "my-shop".into(), instance: None });
        assert_eq!(a.to_string(), "appmcp://my-shop");
        let b = Address::parse("appmcp://my-shop/w2").unwrap();
        assert_eq!(b.instance.as_deref(), Some("w2"));
        assert_eq!(b.to_string(), "appmcp://my-shop/w2");
    }

    #[test]
    fn parse_rejects_non_canonical() {
        let long_inst = format!("appmcp://shop/{}", "a".repeat(33));
        for bad in [
            "",
            "appmcp://",
            "APPMCP://shop",
            "appmcp://Shop",
            "appmcp://shop/",
            "appmcp://shop:80",
            "appmcp://shop?x=1",
            "appmcp://shop#f",
            "appmcp://shop/w2/x",
            "appmcp://shop/default",
            "appmcp://shop/2w",
            "appmcp://user@shop",
            "appmcp://sh%6Fp",
            "app-mcp://shop",
            long_inst.as_str(),
        ] {
            assert!(Address::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn dbus_mapping_round_trips() {
        let a = Address::parse("appmcp://my-shop/w-2").unwrap();
        assert_eq!(dbus::bus_name(&a), "dev.appmcp.App.my_shop.w_2");
        assert_eq!(dbus::object_path(&a), "/dev/appmcp/App/w_2");
        assert_eq!(dbus::parse_bus_name("dev.appmcp.App.my_shop.w_2"), Some(a));
        let d = Address::parse("appmcp://shop").unwrap();
        assert_eq!(dbus::bus_name(&d), "dev.appmcp.App.shop");
        assert_eq!(dbus::object_path(&d), "/dev/appmcp/App");
        assert_eq!(dbus::parse_bus_name("dev.appmcp.App.shop"), Some(d));
    }

    #[test]
    fn dbus_parse_rejects_foreign_names() {
        for bad in [
            "org.freedesktop.DBus",
            ":1.42",
            "dev.appmcp.App.",
            "dev.appmcp.App.Shop",
            "dev.appmcp.App.my-shop",
            "dev.appmcp.App.shop.default",
            "dev.appmcp.App.shop.a.b",
            "dev.appmcp.App.1shop",
        ] {
            assert_eq!(dbus::parse_bus_name(bad), None, "{bad}");
        }
    }

    #[test]
    fn pipe_names_use_dot_for_instance() {
        let sid = "S-1-5-21-1-2-3-1001";
        let d = Address::parse("appmcp://my-shop").unwrap();
        assert_eq!(pipe::pipe_name(sid, &d), r"\\.\pipe\appmcp-S-1-5-21-1-2-3-1001-my-shop");
        let i = Address::parse("appmcp://my-shop/w-2").unwrap();
        assert_eq!(pipe::pipe_name(sid, &i), r"\\.\pipe\appmcp-S-1-5-21-1-2-3-1001-my-shop.w-2");
        // 最长的地址仍在管道名上限内（spec/naming.md 4.3）。
        let long = Address::new(&format!("a{}", "b".repeat(62)), Some(&format!("c{}", "d".repeat(31)))).unwrap();
        let sid_max = "S-1-5-21-4294967295-4294967295-4294967295-4294967295";
        assert!(crate::endpoint::check_pipe_name(&pipe::pipe_name(sid_max, &long)).is_ok());
    }

    #[test]
    fn refusal_lines_round_trip() {
        let line = pipe::refusal_line(codes::CHANNEL_LIMIT, "App 已有连接\n第二行");
        assert_eq!(line, "CHANNEL_LIMIT：App 已有连接 第二行");
        assert_eq!(pipe::parse_refusal(&format!("{line}\n")), (codes::CHANNEL_LIMIT, "App 已有连接 第二行".to_owned()));
        assert_eq!(pipe::parse_refusal("ACTIVATION_DENIED: stopped"), (codes::ACTIVATION_DENIED, "stopped".to_owned()));
        assert_eq!(pipe::parse_refusal("WHATEVER：x").0, codes::ACTIVATION_DENIED);
        assert_eq!(pipe::parse_refusal("").0, codes::ACTIVATION_DENIED);
        let long = pipe::refusal_line(codes::CHANNEL_LIMIT, &"长".repeat(400));
        assert!(long.len() < pipe::MAX_REFUSAL_LINE && long.starts_with("CHANNEL_LIMIT："));
        assert_eq!(codes::lookup("HUB_NOT_TRUSTED"), Some(codes::HUB_NOT_TRUSTED));
        assert_eq!(codes::lookup("nope"), None);
    }

    #[test]
    fn executable_paths_compare_canonically() {
        assert!(pipe::same_executable(r"\\?\C:\Apps\Shop.exe", r"c:\apps\shop.EXE"));
        assert!(pipe::same_executable("C:/Apps/Shop.exe", r"C:\Apps\Shop.exe"));
        assert!(pipe::same_executable(r"\\?\UNC\srv\share\a.exe", r"\\srv\share\a.exe"));
        assert!(!pipe::same_executable(r"C:\Apps\Shop.exe", r"C:\Apps\Other.exe"));
        assert_eq!(pipe::strip_verbatim(r"\\?\C:\Apps\Shop.exe"), r"C:\Apps\Shop.exe");
        assert_eq!(pipe::strip_verbatim("/opt/shop"), "/opt/shop");
    }

    #[test]
    fn registration_parse_validates() {
        let ok = r#"{"registrationVersion":1,"appId":"shop","name":"店","source":"manual",
            "executable":"C:\\a\\shop.exe","activation":{"kind":"exec","target":""}}"#;
        let reg = registration::parse(ok, "shop").unwrap();
        assert_eq!(reg.activation.kind, registration::kinds::EXEC);
        assert_eq!(reg.executable.as_deref(), Some(r"C:\a\shop.exe"));
        assert_eq!(reg.manifest, None);
        // 序列化往返（app install 写出、Hub 读入同一类型）。
        let text = serde_json::to_string(&reg).unwrap();
        assert!(!text.contains("manifest"), "{text}");
        assert_eq!(registration::parse(&text, "shop").unwrap(), reg);
        assert!(registration::parse(ok, "other").is_err(), "文件名不一致");
        assert!(registration::parse(&ok.replace(":1,", ":2,"), "shop").is_err(), "版本");
        assert!(registration::parse(&ok.replace("\"shop\"", "\"Shop\""), "Shop").is_err(), "appId 格式");
        assert!(registration::parse("{", "shop").is_err());
        assert!(registration::parse(r#"{"registrationVersion":1,"appId":"shop","source":"x"}"#, "shop").is_err(), "缺 activation");
        assert_eq!(registration::file_name("shop"), "shop.json");
    }

    #[test]
    fn service_file_quotes_exec() {
        let text = dbus::service_file("my-shop", "/opt/my shop/bin/\"x\"");
        assert_eq!(dbus::service_file_name("my-shop"), "dev.appmcp.App.my_shop.service");
        assert!(text.starts_with("[D-BUS Service]\nName=dev.appmcp.App.my_shop\n"));
        assert!(text.contains("Exec=\"/opt/my shop/bin/\\\"x\\\"\" --app-mcp-activation\n"));
    }
}
