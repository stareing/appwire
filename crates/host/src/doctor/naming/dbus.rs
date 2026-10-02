//! `naming.dbus`（Linux）：会话总线可达、`dev.appmcp.App.*` 激活文件与总线上的名字（spec/naming.md 4.1）。
//!
//! 探测只读：扫描激活目录；经 `DbusConnector::discover`（`ListActivatableNames` + `ListNames`，不触发激活）取名字列表。

use std::path::{Path, PathBuf};

use app_mcp_hub::connector::DiscoveredName;
use app_mcp_protocol::naming::{codes, dbus as names};
use serde::Serialize;
use serde_json::json;

use super::{Finding, NamingEnv, joined_hints, with_naming_code, worst};
use crate::doctor::{Check, Level};

pub const ID: &str = "naming.dbus";
const TITLE: &str = "名字服务：D-Bus";

/// 每个目录最多检查的激活文件数（B-07）。
const MAX_FILES: usize = 256;
/// 单个激活文件的大小上限（B-07）。
const MAX_FILE_BYTES: u64 = 16 * 1024;

const REINSTALL_HINT: &str = "重新登记：app-mcp-host app install --app-id <appId> --exec <程序>";
const RELOAD_HINT: &str =
    "通知总线重新读取激活目录：busctl --user call org.freedesktop.DBus /org/freedesktop/DBus org.freedesktop.DBus ReloadConfig（仍不出现时重新登录）";
const BUS_HINT: &str = "确认登录会话有用户总线：echo $DBUS_SESSION_BUS_ADDRESS、systemctl --user status dbus（WSL 需启用 systemd，或 eval $(dbus-launch --sh-syntax)）；\
不使用按名寻址（serve 未加 --name-service）时可忽略";

/// 一个激活文件。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceFile {
    pub path: PathBuf,
    /// `Name=`。
    pub name: Option<String>,
    /// `Exec=` 原文。
    pub exec: Option<String>,
    /// `Exec` 的程序（第一个参数，去引号）。
    pub program: Option<PathBuf>,
    /// 程序存在且是文件（探测时求得）。
    pub program_exists: bool,
    /// 读取失败的原因。
    pub error: Option<String>,
}

/// 激活文件中 `[D-BUS Service]` 段的 `Name=` 与 `Exec=`。
pub fn parse_service_file(text: &str) -> (Option<String>, Option<String>) {
    let mut in_section = false;
    let (mut name, mut exec) = (None, None);
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_section = line == "[D-BUS Service]";
            continue;
        }
        if !in_section {
            continue;
        }
        if let Some(v) = line.strip_prefix("Name=") {
            name = Some(v.trim().to_owned());
        } else if let Some(v) = line.strip_prefix("Exec=") {
            exec = Some(v.trim().to_owned());
        }
    }
    (name, exec)
}

/// `Exec` 的第一个参数（与 `names::service_file` 的写法对应：双引号包裹，`\` 转义；也接受单引号与不带引号）。
pub fn exec_program(exec: &str) -> Option<String> {
    let mut chars = exec.trim_start().chars();
    let mut out = String::new();
    let mut quote: Option<char> = None;
    if let q @ ('"' | '\'') = chars.clone().next()? {
        quote = Some(q);
        chars.next();
    }
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => return Some(out),
            (Some('\''), c) => out.push(c),
            (_, '\\') => out.push(chars.next()?),
            (None, c) if c.is_whitespace() => break,
            (_, c) => out.push(c),
        }
    }
    // 引号未闭合：不合法。
    if quote.is_some() { None } else { (!out.is_empty()).then_some(out) }
}

/// 读取并解析一个激活文件。
pub fn read_service_file(path: &Path) -> ServiceFile {
    let mut f = ServiceFile { path: path.to_owned(), ..Default::default() };
    let text = match std::fs::metadata(path) {
        Ok(m) if m.len() > MAX_FILE_BYTES => {
            f.error = Some(format!("文件过大（{} 字节）", m.len()));
            return f;
        }
        Ok(_) => std::fs::read_to_string(path),
        Err(e) => Err(e),
    };
    let text = match text {
        Ok(t) => t,
        Err(e) => {
            f.error = Some(e.to_string());
            return f;
        }
    };
    let (name, exec) = parse_service_file(&text);
    f.program = exec.as_deref().and_then(exec_program).map(PathBuf::from);
    f.program_exists = f.program.as_deref().is_some_and(|p| p.is_absolute() && p.is_file());
    f.name = name;
    f.exec = exec;
    f
}

/// 扫描激活目录中的 `dev.appmcp.App.*.service`（用户级在前；同名文件只认第一个，与 dbus-daemon 的优先级一致）。
pub fn scan(dirs: &[PathBuf]) -> Vec<ServiceFile> {
    let mut seen: Vec<std::ffi::OsString> = Vec::new();
    let mut out = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        let mut files: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(names::BUS_NAME_PREFIX) && n.ends_with(".service"))
            })
            .collect();
        files.sort();
        for path in files.into_iter().take(MAX_FILES) {
            let Some(file_name) = path.file_name().map(ToOwned::to_owned) else { continue };
            if seen.contains(&file_name) {
                continue;
            }
            seen.push(file_name);
            out.push(read_service_file(&path));
        }
    }
    out
}

/// 激活文件规则的输入。
struct RuleInput<'a> {
    file: &'a ServiceFile,
    /// 总线可达时的可激活名字。
    activatable: Option<&'a [String]>,
}

type Rule = fn(&RuleInput) -> Option<Finding>;

/// 激活文件规则表（按顺序，第一条命中即停：前面的缺陷会让后面的核对失去意义）。
const RULES: &[Rule] = &[rule_readable, rule_name, rule_name_matches_file, rule_exec, rule_program, rule_activation_arg, rule_bus_knows];

fn label(f: &ServiceFile) -> String {
    f.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn rule_readable(i: &RuleInput) -> Option<Finding> {
    let e = i.file.error.as_deref()?;
    Some(Finding::new(Level::Error, format!("{} 无法读取：{e}", label(i.file))).hint(REINSTALL_HINT))
}

fn rule_name(i: &RuleInput) -> Option<Finding> {
    let Some(name) = i.file.name.as_deref() else {
        return Some(Finding::new(Level::Error, format!("{} 缺少 [D-BUS Service] 段的 Name=", label(i.file))).hint(REINSTALL_HINT));
    };
    match names::parse_bus_name(name) {
        Some(a) if a.instance.is_none() => None,
        Some(_) => Some(Finding::new(Level::Warn, format!("{name} 是实例名字：实例名字不写激活文件（spec/naming.md 4.1）"))
            .hint("删除该激活文件；实例名字由运行中的进程自行登记")),
        None => Some(
            Finding::new(Level::Error, format!("{} 的 Name={name} 不是合法的 dev.appmcp.App.<appId>", label(i.file)))
                .hint(REINSTALL_HINT),
        ),
    }
}

fn rule_name_matches_file(i: &RuleInput) -> Option<Finding> {
    let name = i.file.name.as_deref()?;
    let want = format!("{name}.service");
    (label(i.file) != want)
        .then(|| Finding::new(Level::Warn, format!("{} 的文件名应为 {want}（与 Name 一致）", label(i.file))).hint(REINSTALL_HINT))
}

fn rule_exec(i: &RuleInput) -> Option<Finding> {
    match (&i.file.exec, &i.file.program) {
        (None, _) => Some(Finding::new(Level::Error, format!("{} 缺少 Exec=", label(i.file))).hint(REINSTALL_HINT)),
        (Some(e), None) => Some(Finding::new(Level::Error, format!("{} 的 Exec 无法解析：{e}", label(i.file))).hint(REINSTALL_HINT)),
        _ => None,
    }
}

fn rule_program(i: &RuleInput) -> Option<Finding> {
    let p = i.file.program.as_deref()?;
    (!i.file.program_exists).then(|| {
        Finding::new(
            Level::Error,
            format!("{} 的程序 {} 不存在或不是绝对路径（激活失败，调用报 APP_NOT_INSTALLED）", label(i.file), p.display()),
        )
        .hint(REINSTALL_HINT)
    })
}

fn rule_activation_arg(i: &RuleInput) -> Option<Finding> {
    let exec = i.file.exec.as_deref()?;
    (!exec.split_whitespace().any(|a| a == names::ACTIVATION_ARG)).then(|| {
        Finding::new(
            Level::Warn,
            format!("{} 的 Exec 未带 {}：App 不知道自己由激活冷启动，通道关闭后不会退出", label(i.file), names::ACTIVATION_ARG),
        )
        .hint(REINSTALL_HINT)
    })
}

fn rule_bus_knows(i: &RuleInput) -> Option<Finding> {
    let (name, activatable) = (i.file.name.as_deref()?, i.activatable?);
    (!activatable.iter().any(|n| n == name))
        .then(|| Finding::new(Level::Warn, format!("总线的可激活名字中没有 {name}（激活文件写入后总线尚未重新读取）")).hint(RELOAD_HINT))
}

/// 总线上的名字（`discover` 的结果）。
pub type BusNames = Result<Vec<DiscoveredName>, String>;

/// 汇总为一项检查。
pub fn evaluate(bus: &BusNames, files: &[ServiceFile]) -> Check {
    let activatable: Option<Vec<String>> = bus.as_ref().ok().map(|names_| {
        names_.iter().filter(|n| n.activatable).map(|n| names::bus_name(&n.address)).collect()
    });
    let mut findings: Vec<Finding> = files
        .iter()
        .filter_map(|file| {
            let input = RuleInput { file, activatable: activatable.as_deref() };
            RULES.iter().find_map(|rule| rule(&input))
        })
        .collect();
    let mut lines = Vec::new();
    match bus {
        Err(e) => {
            let level = if files.is_empty() { Level::Info } else { Level::Warn };
            findings.insert(0, Finding::new(level, format!("会话总线不可达：{e}")).hint(BUS_HINT));
        }
        Ok(found) => {
            lines.push("会话总线可达".to_owned());
            for n in found {
                let state = match (n.activatable, n.running) {
                    (_, true) => "运行中（名字已被拥有）",
                    (true, false) => "可激活（未运行）",
                    (false, false) => "未运行",
                };
                lines.push(format!("{}：{state}", n.detail));
            }
        }
    }
    let details = json!({
        "bus": match bus {
            Ok(found) => json!({ "reachable": true, "names": found.iter().map(|n| json!({
                "name": n.detail, "appId": n.address.app_id, "instance": n.address.instance,
                "activatable": n.activatable, "running": n.running,
            })).collect::<Vec<_>>() }),
            Err(e) => json!({ "reachable": false, "error": e }),
        },
        "serviceFiles": files,
        "findings": findings,
    });
    let no_names = bus.as_ref().is_ok_and(Vec::is_empty);
    if files.is_empty() && no_names {
        return Check::new(ID, TITLE, Level::Info, "会话总线可达；没有 dev.appmcp.App.* 激活文件，总线上也没有该前缀的名字")
            .hint("需要按名寻址的 App 用 app-mcp-host app install 登记，或由 App 的安装程序写入激活文件")
            .details(details);
    }
    let level = worst(&findings, Level::Ok);
    lines.extend(findings.iter().map(|f| f.text.clone()));
    let mut c = Check::new(ID, TITLE, level, lines.join("；")).details(details);
    c.hint = joined_hints(&findings);
    if findings.iter().any(|f| f.level == Level::Error && f.text.contains("APP_NOT_INSTALLED")) {
        c = with_naming_code(c, codes::NAME_NOT_FOUND);
    }
    c
}

/// 经 Hub 的 D-Bus 连接器列出名字（有超时；总线缺失时快速失败）。
#[cfg(target_os = "linux")]
async fn bus_names(env: &NamingEnv) -> BusNames {
    use app_mcp_hub::connector::{Connector, DbusConnector};
    let connector = DbusConnector::new(env.dbus_address.clone());
    match tokio::time::timeout(env.timeout, connector.discover()).await {
        Ok(Ok(found)) => Ok(found),
        Ok(Err(e)) => Err(e.message),
        Err(_) => Err(format!("{} 秒内没有回复", env.timeout.as_secs_f32())),
    }
}

/// 运行检查。
#[cfg(target_os = "linux")]
pub async fn check(env: &NamingEnv) -> Check {
    let files = scan(&env.dbus_service_dirs);
    let bus = bus_names(env).await;
    evaluate(&bus, &files)
}

/// 非 Linux：检查表不会选中本项（保留同名入口以便表在各平台一致）。
#[cfg(not(target_os = "linux"))]
pub async fn check(_: &NamingEnv) -> Check {
    Check::new(ID, TITLE, Level::Skip, "只在 Linux 上检查")
}

#[cfg(test)]
mod tests {
    use super::*;
    use app_mcp_protocol::naming::Address;

    fn name(app: &str, activatable: bool, running: bool) -> DiscoveredName {
        let address = Address::new(app, None).unwrap();
        DiscoveredName { detail: names::bus_name(&address), address, activatable, running }
    }

    fn good_file() -> ServiceFile {
        ServiceFile {
            path: PathBuf::from("/x/dbus-1/services/dev.appmcp.App.my_shop.service"),
            name: Some("dev.appmcp.App.my_shop".into()),
            exec: Some("\"/opt/my shop/bin\" --app-mcp-activation".into()),
            program: Some(PathBuf::from("/opt/my shop/bin")),
            program_exists: true,
            error: None,
        }
    }

    #[test]
    fn parse_service_file_and_exec() {
        let text = names::service_file("my-shop", "/opt/my \"q\" shop\\bin");
        let (n, e) = parse_service_file(&text);
        assert_eq!(n.as_deref(), Some("dev.appmcp.App.my_shop"));
        assert_eq!(exec_program(e.as_deref().unwrap()).as_deref(), Some("/opt/my \"q\" shop\\bin"), "与写入方的转义互逆");
        assert_eq!(exec_program("/usr/bin/app --x").as_deref(), Some("/usr/bin/app"));
        assert_eq!(exec_program("'/a b/c' --x").as_deref(), Some("/a b/c"));
        assert_eq!(exec_program("\"/unterminated"), None);
        assert_eq!(exec_program("   "), None);
        let (n, e) = parse_service_file("[Other]\nName=x\n[D-BUS Service]\nExec=/bin/true\n");
        assert_eq!((n, e.as_deref()), (None, Some("/bin/true")), "只读 [D-BUS Service] 段");
    }

    #[test]
    fn healthy_bus_and_file_is_ok() {
        let bus = Ok(vec![name("my-shop", true, false), name("notes", false, true)]);
        let c = evaluate(&bus, &[good_file()]);
        assert_eq!(c.status, Level::Ok, "{}", c.summary);
        assert!(c.summary.contains("dev.appmcp.App.my_shop：可激活（未运行）") && c.summary.contains("dev.appmcp.App.notes：运行中"), "{}", c.summary);
    }

    /// 每条规则各破坏一处（T-09）。
    #[test]
    fn each_rule_detects_its_defect() {
        let bus: BusNames = Ok(vec![name("my-shop", true, false)]);
        type Mutate = fn(&mut ServiceFile);
        let cases: &[(Mutate, Level, &str)] = &[
            (|f| f.error = Some("权限不足".into()), Level::Error, "无法读取"),
            (|f| f.name = None, Level::Error, "缺少 [D-BUS Service] 段的 Name="),
            (|f| f.name = Some("org.example.Foo".into()), Level::Error, "不是合法的"),
            (|f| f.name = Some("dev.appmcp.App.my_shop.w2".into()), Level::Warn, "实例名字"),
            (|f| f.path = PathBuf::from("/x/dev.appmcp.App.other.service"), Level::Warn, "文件名应为 dev.appmcp.App.my_shop.service"),
            (|f| { f.exec = None; f.program = None; }, Level::Error, "缺少 Exec="),
            (|f| f.program = None, Level::Error, "Exec 无法解析"),
            (|f| f.program_exists = false, Level::Error, "不存在或不是绝对路径"),
            (|f| f.exec = Some("\"/opt/my shop/bin\"".into()), Level::Warn, "未带 --app-mcp-activation"),
        ];
        for (mutate, level, needle) in cases {
            let mut f = good_file();
            mutate(&mut f);
            let c = evaluate(&bus, &[f]);
            assert_eq!(c.status, *level, "{needle}: {}", c.summary);
            assert!(c.summary.contains(needle), "{needle}: {}", c.summary);
            assert!(c.hint.is_some(), "{needle}: 有修复建议");
        }
        // 可激活列表中没有该名字。
        let c = evaluate(&Ok(vec![]), &[good_file()]);
        assert_eq!(c.status, Level::Warn);
        assert!(c.summary.contains("尚未重新读取") && c.hint.as_deref().is_some_and(|h| h.contains("ReloadConfig")), "{c:?}");
        // 程序缺失带错误码。
        let mut f = good_file();
        f.program_exists = false;
        assert_eq!(evaluate(&bus, &[f]).code, Some(codes::NAME_NOT_FOUND));
    }

    #[test]
    fn unreachable_bus_and_empty_cases() {
        let down: BusNames = Err("无法连接 D-Bus 会话总线：No such file".into());
        let c = evaluate(&down, &[]);
        assert_eq!(c.status, Level::Info, "没有激活文件时总线缺失只是信息");
        assert!(c.summary.contains("会话总线不可达") && c.hint.as_deref().is_some_and(|h| h.contains("DBUS_SESSION_BUS_ADDRESS")));
        let c = evaluate(&down, &[good_file()]);
        assert_eq!(c.status, Level::Warn, "有激活文件而总线不可达");
        assert!(!c.summary.contains("尚未重新读取"), "总线不可达时不核对可激活列表");
        let c = evaluate(&Ok(vec![]), &[]);
        assert_eq!(c.status, Level::Info);
        assert!(c.summary.contains("没有 dev.appmcp.App.* 激活文件"));
    }

    #[test]
    fn scan_reads_prefixed_files_with_user_priority() {
        let dir = std::env::temp_dir().join(format!("app-mcp-doctor-dbus-{:032x}", rand::random::<u128>()));
        let (user, system) = (dir.join("user"), dir.join("system"));
        std::fs::create_dir_all(&user).unwrap();
        std::fs::create_dir_all(&system).unwrap();
        let exe = dir.join("app");
        std::fs::write(&exe, b"x").unwrap();
        std::fs::write(user.join("dev.appmcp.App.a.service"), names::service_file("a", exe.to_str().unwrap())).unwrap();
        std::fs::write(system.join("dev.appmcp.App.a.service"), "broken").unwrap();
        std::fs::write(system.join("dev.appmcp.App.b.service"), names::service_file("b", "/nonexistent/app-mcp/b")).unwrap();
        std::fs::write(system.join("org.other.service"), "x").unwrap();
        let found = scan(&[user, system, dir.join("missing")]);
        assert_eq!(found.len(), 2);
        assert!(found[0].program_exists && found[0].name.as_deref() == Some("dev.appmcp.App.a"));
        assert!(!found[1].program_exists);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn missing_bus_fails_fast() {
        let env = NamingEnv {
            registration_dirs: vec![],
            dbus_service_dirs: vec![],
            dbus_address: Some("unix:path=/nonexistent/app-mcp-doctor-bus".into()),
            adb: None,
            timeout: std::time::Duration::from_secs(5),
        };
        let started = std::time::Instant::now();
        let c = check(&env).await;
        assert_eq!(c.status, Level::Info, "{}", c.summary);
        assert!(c.summary.contains("会话总线不可达"), "{}", c.summary);
        assert!(started.elapsed() < std::time::Duration::from_secs(4));
    }
}
