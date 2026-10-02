//! `naming.registrations`：桌面平台的 App 登记文件（spec/naming.md 5.3）。
//!
//! 逐个读取登记目录中的 `<appId>.json`：格式、版本与文件名由 `app_mcp_protocol::naming::registration::parse`（唯一定义）校验，
//! 再按规则表核对可执行文件、清单与激活方式（激活方式按 kind 查表）。

use std::path::{Path, PathBuf};

use app_mcp_protocol::naming::registration::{self, Registration, kinds};
use app_mcp_protocol::naming::{Address, dbus as names};
use serde_json::json;
use sha2::{Digest, Sha256};

use super::{Finding, NamingEnv, Platform, joined_hints, worst};
use crate::doctor::{Check, Level};

pub const ID: &str = "naming.registrations";
const TITLE: &str = "名字服务：App 登记文件";

/// 每个目录最多检查的登记文件数（B-07）。
const MAX_FILES: usize = 256;
/// 单个登记文件的大小上限（B-07）。
const MAX_FILE_BYTES: u64 = 64 * 1024;

pub(super) const REINSTALL_HINT: &str = "重新登记：app-mcp-host app install --app-id <appId> --exec <程序>；不再使用时 app-mcp-host app uninstall --app-id <appId>";

/// 一个登记文件的检查结果。
#[derive(Debug)]
pub struct RegistrationFinding {
    pub path: PathBuf,
    pub app_id: Option<String>,
    pub source: Option<String>,
    pub findings: Vec<Finding>,
    /// 通过格式校验的登记（其他检查据此核对平台上的名字，如 `naming.pipes`）。
    pub registration: Option<Registration>,
}

/// 规则的输入（已通过 [`registration::parse`] 的格式校验）。
struct RuleInput<'a> {
    reg: &'a Registration,
    platform: Platform,
    dbus_service_dirs: &'a [PathBuf],
}

type Rule = fn(&RuleInput) -> Option<Finding>;

/// 登记文件规则表（格式、版本、appId 与文件名一致由 [`registration::parse`] 负责，这里核对它引用的外部事物）。
const RULES: &[Rule] = &[rule_executable, rule_manifest, rule_activation];

/// 绝对路径且是文件；否则给出原因。
pub(super) fn missing_file(path: &str) -> Option<&'static str> {
    let p = Path::new(path);
    if !p.is_absolute() {
        return Some("不是绝对路径");
    }
    (!p.is_file()).then_some("不存在")
}

fn rule_executable(i: &RuleInput) -> Option<Finding> {
    let exe = i.reg.executable.as_deref()?;
    let why = missing_file(exe)?;
    Some(Finding::new(Level::Error, format!("程序 {exe} {why}（App 已卸载或移动）")).hint(REINSTALL_HINT))
}

fn rule_manifest(i: &RuleInput) -> Option<Finding> {
    let path = i.reg.manifest.as_deref()?;
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            return Some(
                Finding::new(Level::Warn, format!("清单 {path} 无法读取：{e}"))
                    .hint("App 未运行时将列不出工具；带 --manifest 重新 app install"),
            );
        }
    };
    let want = i.reg.manifest_sha256.as_deref()?;
    let got = format!("{:x}", Sha256::digest(&bytes));
    (!got.eq_ignore_ascii_case(want)).then(|| {
        Finding::new(Level::Warn, format!("清单 {path} 的内容与 manifestSha256 不一致（登记后被修改）"))
            .hint("带 --manifest 重新 app install 以更新摘要")
    })
}

/// 激活方式规则表：kind → 核对（[`kinds`]）。
const ACTIVATION_RULES: &[(&str, Rule)] = &[
    (kinds::DBUS, activation_dbus),
    (kinds::EXEC, activation_exec),
    (kinds::URI, activation_target_required),
    (kinds::AUMID, activation_aumid),
    (kinds::LAUNCHD, activation_launchd),
    (kinds::NONE, activation_none),
];

fn rule_activation(i: &RuleInput) -> Option<Finding> {
    let kind = i.reg.activation.kind.as_str();
    match ACTIVATION_RULES.iter().find(|(k, _)| *k == kind) {
        Some((_, rule)) => rule(i),
        None => Some(
            Finding::new(Level::Warn, format!("未知的激活方式「{kind}」：按不可激活处理（App 未运行时不能按名调用）"))
                .hint(REINSTALL_HINT),
        ),
    }
}

fn activation_dbus(i: &RuleInput) -> Option<Finding> {
    if i.platform != Platform::Linux {
        return Some(Finding::new(Level::Warn, "激活方式 dbus 只在 Linux 有效").hint(REINSTALL_HINT));
    }
    let address = Address::new(&i.reg.app_id, None).ok()?;
    let expected = names::bus_name(&address);
    if i.reg.activation.target != expected {
        return Some(
            Finding::new(Level::Warn, format!("激活目标应为 {expected}，实际为「{}」", i.reg.activation.target))
                .hint(REINSTALL_HINT),
        );
    }
    let file = names::service_file_name(&i.reg.app_id);
    let found = i.dbus_service_dirs.iter().any(|d| d.join(&file).is_file());
    (!found).then(|| {
        Finding::new(Level::Error, format!("缺少激活文件 {file}（App 未运行时无法激活，调用报 APP_NOT_INSTALLED）")).hint(REINSTALL_HINT)
    })
}

/// `exec` 激活运行的程序：`target`，为空时为 `executable`（spec/naming.md 4.3）。
pub(super) fn exec_program(reg: &Registration) -> Option<&str> {
    Some(reg.activation.target.as_str()).filter(|t| !t.is_empty()).or(reg.executable.as_deref())
}

fn activation_exec(i: &RuleInput) -> Option<Finding> {
    let Some(target) = exec_program(i.reg) else {
        return Some(Finding::new(Level::Error, "激活方式 exec 既没有 target 也没有 executable").hint(REINSTALL_HINT));
    };
    let why = missing_file(target)?;
    Some(Finding::new(Level::Error, format!("激活程序 {target} {why}（调用报 APP_NOT_INSTALLED）")).hint(REINSTALL_HINT))
}

fn activation_target_required(i: &RuleInput) -> Option<Finding> {
    i.reg.activation.target.is_empty().then(|| Finding::new(Level::Error, "激活方式缺少 target").hint(REINSTALL_HINT))
}

fn activation_aumid(i: &RuleInput) -> Option<Finding> {
    activation_target_required(i)
        .or_else(|| Some(Finding::new(Level::Info, "aumid 激活未核对（打包 App 是否已安装需在 Windows 上查看「开始」菜单或 Get-AppxPackage）")))
}

/// `launchd`（4.4）：只在 macOS 有效，`target` 为套接字的绝对路径；作业与套接字由 `naming.launchd` 核对。
fn activation_launchd(i: &RuleInput) -> Option<Finding> {
    if i.platform != Platform::MacOs {
        return Some(Finding::new(Level::Warn, "激活方式 launchd 只在 macOS 有效").hint(REINSTALL_HINT));
    }
    (!app_mcp_protocol::endpoint::is_unix_absolute(&i.reg.activation.target)).then(|| {
        Finding::new(Level::Error, format!("激活目标「{}」不是套接字的绝对路径", i.reg.activation.target)).hint(REINSTALL_HINT)
    })
}

fn activation_none(_: &RuleInput) -> Option<Finding> {
    Some(Finding::new(Level::Info, "没有激活方式：App 未运行时不能按名调用"))
}

/// 核对一个登记文件的内容（文件名去掉 `.json` 即期望的 appId）。
pub fn inspect(path: &Path, text: &str, platform: Platform, dbus_service_dirs: &[PathBuf]) -> RegistrationFinding {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
    let reg = match registration::parse(text, stem) {
        Ok(r) => r,
        Err(e) => {
            return RegistrationFinding {
                path: path.to_owned(),
                app_id: None,
                source: None,
                findings: vec![Finding::new(Level::Error, format!("{e}；该文件被忽略")).hint(REINSTALL_HINT)],
                registration: None,
            };
        }
    };
    let input = RuleInput { reg: &reg, platform, dbus_service_dirs };
    let findings = RULES.iter().filter_map(|rule| rule(&input)).collect();
    RegistrationFinding {
        path: path.to_owned(),
        app_id: Some(reg.app_id.clone()),
        source: Some(reg.source.clone()),
        findings,
        registration: Some(reg),
    }
}

/// 文件权限：Unix 上须属于当前用户（系统目录可属于 root），且组与其他用户不可写（spec/naming.md 5.3）。
#[cfg(unix)]
fn permission_finding(meta: &std::fs::Metadata) -> Option<Finding> {
    use std::os::unix::fs::MetadataExt;
    let me = app_mcp_protocol::endpoint::current_uid();
    let mode = meta.mode() & 0o777;
    let owner_ok = meta.uid() == me || meta.uid() == 0;
    (!owner_ok || mode & 0o022 != 0).then(|| {
        Finding::new(Level::Error, format!("文件属于 uid {}、权限 {mode:o}：不属于当前用户或组 / 其他用户可写，不会被读取", meta.uid()))
            .hint("chmod 600 该文件，并确认属于当前用户")
    })
}

#[cfg(not(unix))]
fn permission_finding(_: &std::fs::Metadata) -> Option<Finding> {
    None
}

/// 读取一个文件并核对。
fn read_one(path: &Path, platform: Platform, dbus_service_dirs: &[PathBuf]) -> RegistrationFinding {
    let failed = |text: String| RegistrationFinding {
        path: path.to_owned(),
        app_id: None,
        source: None,
        findings: vec![Finding::new(Level::Error, text)],
        registration: None,
    };
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) => return failed(format!("无法读取：{e}")),
    };
    if meta.len() > MAX_FILE_BYTES {
        return failed(format!("文件过大（{} 字节，上限 {MAX_FILE_BYTES}）", meta.len()));
    }
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => return failed(format!("无法读取：{e}")),
    };
    let mut found = inspect(path, &text, platform, dbus_service_dirs);
    found.findings.extend(permission_finding(&meta));
    found
}

/// 扫描登记目录（用户级在前；同名文件只认第一个）。
pub fn scan(dirs: &[PathBuf], platform: Platform, dbus_service_dirs: &[PathBuf]) -> Vec<RegistrationFinding> {
    let mut seen: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        let mut files: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .collect();
        files.sort();
        for path in files.into_iter().take(MAX_FILES) {
            let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            if seen.contains(&stem) {
                continue;
            }
            seen.push(stem);
            out.push(read_one(&path, platform, dbus_service_dirs));
        }
    }
    out
}

/// 汇总为一项检查。
pub fn evaluate(dirs: &[PathBuf], found: &[RegistrationFinding]) -> Check {
    let details = json!({
        "dirs": dirs,
        "registrations": found.iter().map(|r| json!({
            "path": r.path, "appId": r.app_id, "source": r.source, "findings": r.findings,
        })).collect::<Vec<_>>(),
    });
    if found.is_empty() {
        let where_ = dirs.iter().map(|d| d.display().to_string()).collect::<Vec<_>>().join("、");
        return Check::new(ID, TITLE, Level::Info, format!("没有 App 登记文件（{where_}）"))
            .hint("需要按名寻址的未打包 App 用 app-mcp-host app install 登记（spec/naming.md 5.3）")
            .details(details);
    }
    let all = found.iter().flat_map(|r| r.findings.iter());
    let level = worst(all.clone().filter(|f| f.level != Level::Info), Level::Ok);
    let lines: Vec<String> = found
        .iter()
        .map(|r| {
            let who = r.app_id.clone().unwrap_or_else(|| r.path.display().to_string());
            let source = r.source.as_deref().map(|s| format!("，{s}")).unwrap_or_default();
            let problems: Vec<&str> = r.findings.iter().map(|f| f.text.as_str()).collect();
            let state = if problems.is_empty() { "正常".to_owned() } else { problems.join("；") };
            format!("{who}（{}{source}）：{state}", r.path.display())
        })
        .collect();
    let mut c = Check::new(ID, TITLE, level, lines.join("；")).details(details);
    c.hint = joined_hints(all);
    c
}

/// 运行检查。
pub fn check(env: &NamingEnv) -> Check {
    let found = scan(&env.registration_dirs, Platform::current(), &env.dbus_service_dirs);
    evaluate(&env.registration_dirs, &found)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 不存在的绝对路径的目录（各平台的绝对路径形式不同）。
    #[cfg(windows)]
    const GONE: &str = r"C:\nonexistent\app-mcp";
    #[cfg(not(windows))]
    const GONE: &str = "/nonexistent/app-mcp";

    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            let d = std::env::temp_dir().join(format!("app-mcp-doctor-reg-{:032x}", rand::random::<u128>()));
            std::fs::create_dir_all(&d).unwrap();
            Self(d)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// 合法登记：程序、清单（摘要一致）、D-Bus 激活文件都存在。
    fn valid(dir: &Path) -> (PathBuf, serde_json::Value, Vec<PathBuf>) {
        let exe = dir.join("shop-bin");
        std::fs::write(&exe, b"x").unwrap();
        let manifest = dir.join("shop.app-mcp.json");
        std::fs::write(&manifest, b"{}").unwrap();
        let services = dir.join("services");
        std::fs::create_dir_all(&services).unwrap();
        std::fs::write(services.join(names::service_file_name("my-shop")), b"").unwrap();
        let reg = json!({
            "registrationVersion": 1, "appId": "my-shop", "name": "商城", "source": "manual",
            "manifest": manifest, "manifestSha256": format!("{:x}", Sha256::digest(b"{}")),
            "executable": exe, "activation": {"kind": "dbus", "target": "dev.appmcp.App.my_shop"},
        });
        (dir.join("my-shop.json"), reg, vec![services])
    }

    fn texts(r: &RegistrationFinding) -> Vec<(Level, String)> {
        r.findings.iter().map(|f| (f.level, f.text.clone())).collect()
    }

    #[test]
    fn valid_registration_has_no_findings() {
        let s = Scratch::new();
        let (path, reg, services) = valid(&s.0);
        let r = inspect(&path, &reg.to_string(), Platform::Linux, &services);
        assert!(r.findings.is_empty(), "{:?}", texts(&r));
        assert_eq!(r.registration.as_ref().map(|g| g.app_id.as_str()), Some("my-shop"), "其他检查据此核对平台上的名字");
        assert_eq!(evaluate(&[], &[r]).status, Level::Ok);
    }

    /// 每条规则（含每种激活方式与兜底）各破坏一处，断言命中的级别与说明（T-09）。
    #[test]
    fn each_rule_detects_its_defect() {
        let s = Scratch::new();
        let (path, base, services) = valid(&s.0);
        type Mutate = fn(&mut serde_json::Value);
        let cases: &[(&str, Mutate, Level, &str)] = &[
            ("version missing", |r| { r.as_object_mut().unwrap().remove("registrationVersion"); }, Level::Error, "registrationVersion"),
            ("version unknown", |r| r["registrationVersion"] = json!(9), Level::Error, "不支持的 registrationVersion 9"),
            ("appId missing", |r| { r.as_object_mut().unwrap().remove("appId"); }, Level::Error, "appId"),
            ("appId invalid", |r| r["appId"] = json!("Bad Id"), Level::Error, "不合法"),
            ("appId != stem", |r| r["appId"] = json!("other"), Level::Error, "与文件名「my-shop.json」不一致"),
            ("exe relative", |r| r["executable"] = json!("bin/shop"), Level::Error, "不是绝对路径"),
            ("exe gone", |r| r["executable"] = json!(format!("{GONE}/shop")), Level::Error, "app-mcp/shop 不存在"),
            ("manifest gone", |r| r["manifest"] = json!("/nonexistent/app-mcp/m.json"), Level::Warn, "无法读取"),
            ("manifest changed", |r| r["manifestSha256"] = json!("00"), Level::Warn, "不一致"),
            ("activation missing", |r| { r.as_object_mut().unwrap().remove("activation"); }, Level::Error, "activation"),
            ("dbus target", |r| r["activation"]["target"] = json!("dev.appmcp.App.x"), Level::Warn, "激活目标应为 dev.appmcp.App.my_shop"),
            ("exec gone", |r| r["activation"] = json!({"kind": "exec", "target": format!("{GONE}/x")}), Level::Error, "app-mcp/x 不存在"),
            ("exec nothing", |r| { r["activation"] = json!({"kind": "exec"}); r.as_object_mut().unwrap().remove("executable"); }, Level::Error, "既没有 target 也没有 executable"),
            ("uri no target", |r| r["activation"] = json!({"kind": "uri"}), Level::Error, "缺少 target"),
            ("aumid no target", |r| r["activation"] = json!({"kind": "aumid"}), Level::Error, "缺少 target"),
            ("aumid", |r| r["activation"] = json!({"kind": "aumid", "target": "Pub.App_x!App"}), Level::Info, "aumid 激活未核对"),
            ("none", |r| r["activation"] = json!({"kind": "none"}), Level::Info, "没有激活方式"),
            ("launchd off macOS", |r| r["activation"] = json!({"kind": "launchd", "target": "/s.sock"}), Level::Warn, "只在 macOS 有效"),
            ("unknown kind", |r| r["activation"] = json!({"kind": "com", "target": "x"}), Level::Warn, "未知的激活方式「com」"),
        ];
        for (name, mutate, level, needle) in cases {
            let mut reg = base.clone();
            mutate(&mut reg);
            let r = inspect(&path, &reg.to_string(), Platform::Linux, &services);
            assert!(
                r.findings.iter().any(|f| f.level == *level && f.text.contains(needle)),
                "{name}: {:?}",
                texts(&r)
            );
        }
        // 无缺陷的变体：uri 有 target；exec 缺省用 executable（存在）；没有 executable 字段（可省略）。
        type Ok_ = fn(&mut serde_json::Value);
        let fine: &[Ok_] = &[
            |r| r["activation"] = json!({"kind": "uri", "target": "appmcp-shop"}),
            |r| r["activation"] = json!({"kind": "exec"}),
            |r| { r.as_object_mut().unwrap().remove("executable"); },
        ];
        for mutate in fine {
            let mut reg = base.clone();
            mutate(&mut reg);
            let r = inspect(&path, &reg.to_string(), Platform::Linux, &services);
            assert!(r.findings.is_empty(), "{reg}: {:?}", texts(&r));
        }
    }

    #[test]
    fn dbus_activation_needs_service_file_and_linux() {
        let s = Scratch::new();
        let (path, reg, _) = valid(&s.0);
        let r = inspect(&path, &reg.to_string(), Platform::Linux, &[s.0.join("empty")]);
        assert!(r.findings.iter().any(|f| f.level == Level::Error && f.text.contains("缺少激活文件 dev.appmcp.App.my_shop.service")), "{:?}", texts(&r));
        let r = inspect(&path, &reg.to_string(), Platform::Windows, &[]);
        assert!(r.findings.iter().any(|f| f.text.contains("只在 Linux 有效")), "{:?}", texts(&r));
        let r = inspect(&path, "not json", Platform::Linux, &[]);
        assert_eq!(r.findings[0].level, Level::Error);
        assert!(r.registration.is_none());
    }

    #[test]
    fn launchd_activation_needs_absolute_socket_on_macos() {
        let s = Scratch::new();
        let (path, mut reg, _) = valid(&s.0);
        reg["activation"] = json!({"kind": "launchd", "target": "/Users/u/.app-mcp/run/apps/my-shop.sock"});
        let r = inspect(&path, &reg.to_string(), Platform::MacOs, &[]);
        assert!(r.findings.is_empty(), "{:?}", texts(&r));
        reg["activation"]["target"] = json!("my-shop.sock");
        let r = inspect(&path, &reg.to_string(), Platform::MacOs, &[]);
        assert!(r.findings.iter().any(|f| f.level == Level::Error && f.text.contains("不是套接字的绝对路径")), "{:?}", texts(&r));
    }

    #[test]
    fn scan_prefers_user_dir_and_reports_files() {
        let s = Scratch::new();
        let (_, reg, services) = valid(&s.0);
        let user = s.0.join("user");
        let system = s.0.join("system");
        std::fs::create_dir_all(&user).unwrap();
        std::fs::create_dir_all(&system).unwrap();
        std::fs::write(user.join("my-shop.json"), reg.to_string()).unwrap();
        std::fs::write(system.join("my-shop.json"), "broken").unwrap();
        std::fs::write(system.join("readme.txt"), "ignored").unwrap();
        let mut bad = reg.clone();
        bad["executable"] = json!(format!("{GONE}/other"));
        bad["appId"] = json!("other");
        bad["activation"] = json!({"kind": "none"});
        std::fs::write(system.join("other.json"), bad.to_string()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for f in [user.join("my-shop.json"), system.join("other.json")] {
                std::fs::set_permissions(f, std::fs::Permissions::from_mode(0o600)).unwrap();
            }
        }
        let dirs = [user.clone(), system.clone(), s.0.join("missing")];
        let found = scan(&dirs, Platform::Linux, &services);
        assert_eq!(found.len(), 2, "系统目录的同名文件被用户级覆盖，非 .json 忽略");
        assert!(found[0].findings.is_empty(), "{:?}", texts(&found[0]));
        let c = evaluate(&dirs, &found);
        assert_eq!(c.status, Level::Error);
        assert!(c.summary.contains("my-shop") && c.summary.contains("正常") && c.summary.contains(&format!("程序 {GONE}/other 不存在")), "{}", c.summary);
        assert!(c.hint.as_deref().is_some_and(|h| h.contains("app install")));
        assert_eq!(c.details["registrations"].as_array().map(Vec::len), Some(2));

        let empty = evaluate(&dirs, &[]);
        assert_eq!(empty.status, Level::Info);
    }

    #[cfg(unix)]
    #[test]
    fn group_writable_file_is_rejected() {
        use std::os::unix::fs::PermissionsExt;
        let s = Scratch::new();
        let (_, reg, services) = valid(&s.0);
        let dir = s.0.join("apps");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("my-shop.json");
        std::fs::write(&f, reg.to_string()).unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o666)).unwrap();
        let found = scan(std::slice::from_ref(&dir), Platform::Linux, &services);
        assert!(found[0].findings.iter().any(|x| x.level == Level::Error && x.text.contains("不会被读取")), "{:?}", texts(&found[0]));
    }
}
