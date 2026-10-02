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
    ];
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
    fn service_file_quotes_exec() {
        let text = dbus::service_file("my-shop", "/opt/my shop/bin/\"x\"");
        assert_eq!(dbus::service_file_name("my-shop"), "dev.appmcp.App.my_shop.service");
        assert!(text.starts_with("[D-BUS Service]\nName=dev.appmcp.App.my_shop\n"));
        assert!(text.contains("Exec=\"/opt/my shop/bin/\\\"x\\\"\" --app-mcp-activation\n"));
    }
}
