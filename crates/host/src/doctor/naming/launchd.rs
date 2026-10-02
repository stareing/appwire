//! `naming.launchd`：macOS launchd 用户 Agent 的按需套接字（spec/naming.md 4.4）与 App 登记文件的对应。
//!
//! 探测：登记文件（与 `naming.registrations` 同一扫描）中激活方式为 `launchd` 的 App；用户 LaunchAgents 目录中的
//! `dev.appmcp.App.*.plist`；每个 App 的 plist 内容、套接字目录的属主与权限、套接字文件是否存在；`launchctl print gui/<uid>/<label>`
//! 的退出状态（作业是否已载入；只看退出码，不解析输出——launchctl(1)：输出不是接口），每条 5 秒超时。
//! 评估（纯函数）：按规则表给出每个 App 的结论；没有登记的 `dev.appmcp.App.*` plist（Hub 发现不了）。
//!
//! @security 只读：从不 `connect` 套接字（连接即由 launchd 启动 App）、不 `bootstrap` / `bootout`、不改文件。

use std::path::{Path, PathBuf};
use std::time::Duration;

use app_mcp_protocol::naming::launchd as names;
use app_mcp_protocol::naming::registration::{Registration, kinds};
use serde_json::json;

use super::registration::{self, REINSTALL_HINT};
use super::{Finding, NamingEnv, Platform, joined_hints, worst};
use crate::doctor::{Check, Level};

pub const ID: &str = "naming.launchd";
const TITLE: &str = "名字服务：macOS launchd";

/// LaunchAgents 目录中最多检查的 plist 数（B-07）。
const MAX_PLISTS: usize = 1024;
/// 单个 plist 的大小上限（B-07）。
const MAX_PLIST_BYTES: u64 = 64 * 1024;

const LAUNCHCTL: &str = "/bin/launchctl";

/// 一个 `launchd` 登记 App 的探测结果。
#[derive(Debug)]
pub struct AppProbe {
    pub reg: Registration,
    /// plist 内容；不存在 / 读不了时为原因。
    pub plist: Result<String, String>,
    /// 套接字目录的问题（属主 / 权限 / 读不到）；没有问题时为 `None`。
    pub dir_issue: Option<String>,
    /// 套接字文件存在且是套接字（launchd 载入作业时创建）。
    pub socket_present: bool,
    /// `launchctl print` 的结果：`Ok(true)` = 已载入；不能运行 launchctl 时为原因。
    pub loaded: Result<bool, String>,
}

/// 探测结果。
#[derive(Debug)]
pub struct LaunchdProbe {
    pub agents_dir: PathBuf,
    pub apps: Vec<AppProbe>,
    /// LaunchAgents 中 `dev.appmcp.App.<appId>.plist` 的 appId（无论有无登记）。
    pub plists: Vec<String>,
}

type AppRule = fn(&AppProbe) -> Option<Finding>;

/// 每个 App 的规则表：按顺序取第一条命中的结论（C-12）。
const APP_RULES: &[AppRule] = &[app_target, app_plist_missing, app_socket_dir, app_not_loaded, app_socket_missing, app_plist_differs, app_ok];

fn bootstrap_hint(app_id: &str) -> String {
    format!("launchctl bootstrap gui/$UID ~/Library/LaunchAgents/{}，或重新 app install", names::plist_file_name(app_id))
}

fn app_target(p: &AppProbe) -> Option<Finding> {
    let t = &p.reg.activation.target;
    (!app_mcp_protocol::endpoint::is_unix_absolute(t)).then(|| {
        Finding::new(Level::Error, format!("激活目标「{t}」不是套接字的绝对路径：Hub 无法拨号")).hint(REINSTALL_HINT)
    })
}

fn app_plist_missing(p: &AppProbe) -> Option<Finding> {
    let why = p.plist.as_ref().err()?;
    Some(
        Finding::new(Level::Error, format!("Agent plist {why}：launchd 不会创建套接字，调用报 APP_NOT_INSTALLED"))
            .hint(REINSTALL_HINT),
    )
}

fn app_socket_dir(p: &AppProbe) -> Option<Finding> {
    let why = p.dir_issue.as_deref()?;
    Some(
        Finding::new(Level::Error, format!("套接字目录 {why}：Hub 拒绝拨号（PEER_IDENTITY_MISMATCH）"))
            .hint("chmod 700 套接字所在目录，并确认属于当前用户"),
    )
}

fn app_not_loaded(p: &AppProbe) -> Option<Finding> {
    match &p.loaded {
        Ok(true) => None,
        Ok(false) => Some(
            Finding::new(Level::Error, format!("作业 {} 未载入：调用报 APP_NOT_INSTALLED", names::label(&p.reg.app_id)))
                .hint(bootstrap_hint(&p.reg.app_id)),
        ),
        Err(e) => Some(Finding::new(Level::Warn, format!("无法确认作业是否已载入（{e}）"))),
    }
}

fn app_socket_missing(p: &AppProbe) -> Option<Finding> {
    (!p.socket_present).then(|| {
        Finding::new(Level::Warn, format!("作业已载入，但套接字 {} 不存在：plist 中的路径与登记不一致？", p.reg.activation.target))
            .hint(REINSTALL_HINT)
    })
}

fn app_plist_differs(p: &AppProbe) -> Option<Finding> {
    let text = p.plist.as_ref().ok()?;
    let program = p.reg.executable.as_deref().unwrap_or_default();
    let expected = names::agent_plist(&p.reg.app_id, program, &p.reg.activation.target).ok();
    (expected.as_deref() != Some(text.as_str())).then(|| {
        Finding::new(Level::Info, "plist 不是按此登记生成的（安装程序自写或已手改）：未逐项核对程序与套接字路径")
    })
}

fn app_ok(_: &AppProbe) -> Option<Finding> {
    Some(Finding::new(Level::Ok, "已载入；调用时由 launchd 按需启动（是否正在运行不探测：连接即激活）"))
}

/// 汇总为一项检查。
pub fn evaluate(p: &LaunchdProbe) -> Check {
    let mut findings: Vec<Finding> = Vec::new();
    let mut apps = Vec::new();
    for a in &p.apps {
        if let Some(f) = APP_RULES.iter().find_map(|rule| rule(a)) {
            apps.push(json!({
                "appId": a.reg.app_id, "label": names::label(&a.reg.app_id), "socket": a.reg.activation.target,
                "loaded": a.loaded.as_ref().ok(), "socketPresent": a.socket_present, "level": f.level,
            }));
            findings.push(Finding { text: format!("{}：{}", a.reg.app_id, f.text), ..f });
        }
    }
    let registered = |id: &str| p.apps.iter().any(|a| a.reg.app_id == id);
    let orphans: Vec<&str> = p.plists.iter().map(String::as_str).filter(|id| !registered(id)).collect();
    for id in &orphans {
        findings.push(
            Finding::new(Level::Warn, format!("{} 没有 launchd 激活方式的登记文件：Hub 发现不了这个 App", names::plist_file_name(id)))
                .hint(REINSTALL_HINT),
        );
    }
    let details = json!({ "agentsDir": p.agents_dir, "apps": apps, "unregistered": orphans });
    if findings.is_empty() {
        return Check::new(ID, TITLE, Level::Info, "没有以 launchd 激活的 App 登记，也没有 dev.appmcp.App.* Agent")
            .hint("需要按名寻址的非沙盒 App 用 app-mcp-host app install 登记（spec/naming.md 4.4）")
            .details(details);
    }
    let level = worst(findings.iter().filter(|f| f.level != Level::Info), Level::Info);
    let summary = findings.iter().map(|f| f.text.as_str()).collect::<Vec<_>>().join("；");
    let mut c = Check::new(ID, TITLE, level, summary).details(details);
    c.hint = joined_hints(&findings);
    c
}

/// LaunchAgents 中 `dev.appmcp.App.<appId>.plist` 的 appId（最多 [`MAX_PLISTS`] 项）。
fn list_plists(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut ids: Vec<String> = entries
        .take(MAX_PLISTS)
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().into_string().ok())
        .filter_map(|n| {
            let id = n.strip_prefix(names::LABEL_PREFIX)?.strip_suffix(".plist")?;
            app_mcp_protocol::is_valid_app_id(id).then(|| id.to_owned())
        })
        .collect();
    ids.sort();
    ids
}

fn read_plist(path: &Path) -> Result<String, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("{} 无法读取（{e}）", path.display()))?;
    if meta.len() > MAX_PLIST_BYTES {
        return Err(format!("{} 超过 {MAX_PLIST_BYTES} 字节", path.display()));
    }
    std::fs::read_to_string(path).map_err(|e| format!("{} 无法读取（{e}）", path.display()))
}

#[cfg(unix)]
fn socket_facts(socket: &Path) -> (Option<String>, bool) {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    let dir_issue = match socket.parent().map(std::fs::metadata) {
        Some(Ok(m)) => names::socket_dir_issue(m.uid(), m.mode(), app_mcp_protocol::endpoint::current_uid()),
        Some(Err(e)) => Some(format!("无法读取（{e}）")),
        None => Some("没有上级目录".to_owned()),
    };
    let present = std::fs::symlink_metadata(socket).is_ok_and(|m| m.file_type().is_socket());
    (dir_issue, present)
}

#[cfg(not(unix))]
fn socket_facts(_socket: &Path) -> (Option<String>, bool) {
    (Some("本平台没有 Unix 套接字".to_owned()), false)
}

/// `launchctl print gui/<uid>/<label>` 的退出码（0 = 已载入）；不读取输出。
async fn loaded(app_id: &str, timeout: Duration) -> Result<bool, String> {
    #[cfg(unix)]
    {
        let target = format!("gui/{}/{}", app_mcp_protocol::endpoint::current_uid(), names::label(app_id));
        crate::doctor::command::run_tool(Path::new(LAUNCHCTL), &["print", &target], timeout)
            .await
            .map(|o| o.success)
            .map_err(|e| format!("launchctl {e}"))
    }
    #[cfg(not(unix))]
    {
        let _ = (app_id, timeout);
        Err(format!("本平台没有 {LAUNCHCTL}"))
    }
}

/// 运行检查。
pub async fn check(env: &NamingEnv) -> Check {
    let Some(data_home) = crate::data_home::user_data_home() else {
        return Check::new(ID, TITLE, Level::Error, format!("无法确定用户数据目录（{}）", crate::data_home::SOURCE));
    };
    let agents_dir = crate::app_install::LaunchdPaths::agents_dir(&data_home);
    let mut apps = Vec::new();
    for reg in registration::scan(&env.registration_dirs, Platform::current(), &[]).into_iter().filter_map(|r| r.registration) {
        if reg.activation.kind != kinds::LAUNCHD {
            continue;
        }
        let plist = read_plist(&agents_dir.join(names::plist_file_name(&reg.app_id)));
        let (dir_issue, socket_present) = socket_facts(Path::new(&reg.activation.target));
        let loaded = loaded(&reg.app_id, env.timeout).await;
        apps.push(AppProbe { reg, plist, dir_issue, socket_present, loaded });
    }
    evaluate(&LaunchdProbe { plists: list_plists(&agents_dir), agents_dir, apps })
}

#[cfg(test)]
mod tests {
    use super::*;
    use app_mcp_protocol::naming::registration::Activation;

    const SOCK: &str = "/Users/u/.app-mcp/run/apps/shop.sock";
    const EXE: &str = "/Applications/Shop.app/Contents/MacOS/Shop";

    fn reg(app_id: &str, target: &str) -> Registration {
        Registration {
            registration_version: 1,
            app_id: app_id.to_owned(),
            name: String::new(),
            source: "manual".to_owned(),
            manifest: None,
            manifest_sha256: None,
            executable: Some(EXE.to_owned()),
            activation: Activation { kind: kinds::LAUNCHD.to_owned(), target: target.to_owned() },
        }
    }

    /// 一切正常的 App。
    fn healthy() -> AppProbe {
        AppProbe {
            plist: Ok(names::agent_plist("shop", EXE, SOCK).unwrap()),
            reg: reg("shop", SOCK),
            dir_issue: None,
            socket_present: true,
            loaded: Ok(true),
        }
    }

    fn probe(apps: Vec<AppProbe>, plists: &[&str]) -> LaunchdProbe {
        LaunchdProbe { agents_dir: PathBuf::from("/Users/u/Library/LaunchAgents"), apps, plists: plists.iter().map(|s| s.to_string()).collect() }
    }

    /// 每条规则（含兜底）各破坏一处，断言级别与说明（T-09）。
    #[test]
    fn each_rule_decides_its_case() {
        type Mutate = fn(&mut AppProbe);
        let cases: &[(&str, Mutate, Level, &str)] = &[
            ("ok", |_| {}, Level::Ok, "已载入"),
            ("relative target", |p| p.reg.activation.target = "x.sock".into(), Level::Error, "不是套接字的绝对路径"),
            ("plist missing", |p| p.plist = Err("/x 无法读取".into()), Level::Error, "APP_NOT_INSTALLED"),
            ("dir unsafe", |p| p.dir_issue = Some("权限 777".into()), Level::Error, "PEER_IDENTITY_MISMATCH"),
            ("not loaded", |p| p.loaded = Ok(false), Level::Error, "未载入"),
            ("launchctl fails", |p| p.loaded = Err("超时".into()), Level::Warn, "无法确认"),
            ("socket missing", |p| p.socket_present = false, Level::Warn, "套接字"),
            ("plist differs", |p| p.plist = Ok("<plist/>".into()), Level::Info, "不是按此登记生成"),
        ];
        for (name, mutate, level, needle) in cases {
            let mut p = healthy();
            mutate(&mut p);
            let f = APP_RULES.iter().find_map(|r| r(&p)).expect("总有结论");
            assert_eq!(f.level, *level, "{name}: {}", f.text);
            assert!(f.text.contains(needle), "{name}: {}", f.text);
        }
    }

    #[test]
    fn evaluate_summarizes_and_reports_orphans() {
        let c = evaluate(&probe(vec![healthy()], &["shop"]));
        assert_eq!((c.id, c.status), (ID, Level::Ok), "{}", c.summary);

        let mut broken = healthy();
        broken.loaded = Ok(false);
        let c = evaluate(&probe(vec![broken], &["shop", "ghost"]));
        assert_eq!(c.status, Level::Error);
        assert!(c.summary.contains("dev.appmcp.App.ghost.plist 没有"), "{}", c.summary);
        assert!(c.hint.as_deref().is_some_and(|h| h.contains("launchctl bootstrap")), "{:?}", c.hint);
        assert_eq!(c.details["unregistered"], json!(["ghost"]));

        let c = evaluate(&probe(vec![], &[]));
        assert_eq!(c.status, Level::Info);
    }

    #[test]
    fn lists_only_well_formed_agent_plists() {
        let d = std::env::temp_dir().join(format!("app-mcp-doctor-launchd-{:032x}", rand::random::<u128>()));
        std::fs::create_dir_all(&d).unwrap();
        for n in ["dev.appmcp.App.shop.plist", "dev.appmcp.App.Bad.plist", "com.other.plist", "dev.appmcp.App.x.txt"] {
            std::fs::write(d.join(n), b"x").unwrap();
        }
        assert_eq!(list_plists(&d), ["shop"]);
        assert_eq!(list_plists(&d.join("missing")), Vec::<String>::new());
        assert!(read_plist(&d.join("missing")).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn socket_facts_check_dir_and_type() {
        use std::os::unix::fs::PermissionsExt;
        let d = std::env::temp_dir().join(format!("amcp-dl-{:016x}", rand::random::<u64>()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o700)).unwrap();
        let sock = d.join("s.sock");
        assert_eq!(socket_facts(&sock), (None, false));
        let _l = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        assert_eq!(socket_facts(&sock), (None, true));
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o770)).unwrap();
        assert!(socket_facts(&sock).0.is_some_and(|w| w.contains("可写")));
        let _ = std::fs::remove_dir_all(&d);
    }
}
