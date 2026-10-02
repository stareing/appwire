//! `naming.pipes`：Windows 每 App 每用户命名管道（spec/naming.md 4.3）与 App 登记文件的对应。
//!
//! 探测：当前用户 SID、登记文件（与 `naming.registrations` 同一扫描）、`\\.\pipe\` 中以 `appmcp-` 开头的名字（目录查询，有条数上限与超时）。
//! 评估（纯函数）：每个登记 App 的期望管道名、是否在运行（管道存在）、未运行时能否激活（`exec` 程序是否存在）；
//! 当前用户 SID 下没有登记文件的管道（Hub 发现不了）；其他用户 SID 的管道；不符合命名规则的管道。
//!
//! @security 只列名字、从不打开 `appmcp-` 管道：打开即被 App 当作一条通道接受（4.3），会唤醒 App 的连接。因此管道所有者 SID 与
//! 服务端进程映像不在此核对（Hub 拨号时核对，10.3）；以当前用户 SID 命名、却被其他用户抢先创建的管道在这里显示为"运行中"。

use std::path::{Path, PathBuf};
use std::time::Duration;

use app_mcp_protocol::naming::registration::{Registration, kinds};
use app_mcp_protocol::naming::{Address, pipe as names};
use serde_json::json;

use super::registration::{self, REINSTALL_HINT, exec_program, missing_file};
use super::{Finding, NamingEnv, Platform, joined_hints, worst};
use crate::doctor::{Check, Level};

pub const ID: &str = "naming.pipes";
const TITLE: &str = "名字服务：Windows 命名管道";

/// 管道命名空间的目录。
const PIPE_DIR: &str = r"\\.\pipe\";
/// 列出管道时最多读取的目录项（B-07）。
const MAX_PIPE_ENTRIES: usize = 16_384;

const UNREGISTERED_HINT: &str =
    "让 App 的安装程序写登记文件，或 app-mcp-host app install --app-id <appId> --exec <程序>（Windows 上 Hub 只从登记文件发现 App，spec/naming.md 4.3）";

/// 探测结果。
#[derive(Debug)]
pub struct PipeProbe {
    /// 当前用户 SID；取不到时为原因。
    pub sid: Result<String, String>,
    /// `\\.\pipe\` 中以 `appmcp-` 开头的名字（不含目录）；列不出时为原因。
    pub listed: Result<Vec<String>, String>,
    /// 通过格式校验的登记（用户级目录优先，同名只认第一个）。
    pub registrations: Vec<Registration>,
}

/// 目录项中的管道名前缀（`appmcp-`）。
fn listed_prefix() -> &'static str {
    names::PIPE_PREFIX.strip_prefix(PIPE_DIR).unwrap_or(names::PIPE_PREFIX)
}

/// 一条 `appmcp-` 管道的归属。
#[derive(Debug, PartialEq, Eq)]
enum Listed {
    /// 当前用户 SID 下的名字（4.3：`appmcp-<SID>-<appId>[.<instance>]`）。
    Mine { app_id: String, instance: Option<String> },
    /// 其他用户 SID 下的名字。
    Foreign { sid: String },
    /// 不符合命名规则。
    Malformed,
}

/// 按名字归类（管道名不区分大小写）。
fn classify(name: &str, sid: &str) -> Listed {
    let lower = name.to_ascii_lowercase();
    let Some(rest) = lower.strip_prefix(listed_prefix()) else { return Listed::Malformed };
    let mine = format!("{}-", sid.to_ascii_lowercase());
    if let Some(tail) = rest.strip_prefix(&mine) {
        let (app_id, instance) = match tail.split_once('.') {
            Some((a, i)) => (a, Some(i)),
            None => (tail, None),
        };
        return match Address::new(app_id, instance) {
            Ok(a) => Listed::Mine { app_id: a.app_id, instance: a.instance },
            Err(_) => Listed::Malformed,
        };
    }
    foreign_sid(rest).map_or(Listed::Malformed, |sid| Listed::Foreign { sid })
}

/// `s-1-<数字>-…-<appId>` 开头的 SID（大写 `S`）；不是 SID 开头时为 `None`。
///
/// @why appId 以小写字母开头（2.1），SID 各段都是数字，因此第一个非数字段即 SID 的结束。
fn foreign_sid(rest: &str) -> Option<String> {
    let mut parts = rest.split('-');
    let (Some("s"), Some("1")) = (parts.next(), parts.next()) else { return None };
    let numbers: Vec<&str> = parts.take_while(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())).collect();
    (!numbers.is_empty()).then(|| format!("S-1-{}", numbers.join("-")))
}

/// 一个登记 App 的规则输入。
struct AppInput<'a> {
    reg: &'a Registration,
    /// 管道存在；列不出管道时为 `None`。
    running: Option<bool>,
    /// 存在的实例管道数。
    instances: usize,
}

impl AppInput<'_> {
    fn idle(&self) -> &'static str {
        if self.running == Some(false) { "未运行" } else { "运行状态未知" }
    }
}

type AppRule = fn(&AppInput) -> Option<Finding>;

/// 每个登记 App 的规则表：按顺序取第一条命中的结论（C-12）。
const APP_RULES: &[AppRule] = &[app_ignored, app_running, app_exec, app_system_activation, app_not_activatable];

/// 登记的程序已不存在：Hub 忽略这条登记（5.4，与 `PipeConnector` 的判断相同）。
fn app_ignored(i: &AppInput) -> Option<Finding> {
    let exe = i.reg.executable.as_deref().filter(|e| !e.is_empty() && !Path::new(e).exists())?;
    Some(
        Finding::new(Level::Error, format!("登记的程序 {exe} 不存在：Hub 忽略此登记，调用报 APP_NOT_INSTALLED"))
            .hint(REINSTALL_HINT),
    )
}

fn app_running(i: &AppInput) -> Option<Finding> {
    if i.running != Some(true) {
        return None;
    }
    let extra = if i.instances > 0 { format!("，另有 {} 个实例管道", i.instances) } else { String::new() };
    Some(Finding::new(Level::Ok, format!("运行中（管道存在{extra}）")))
}

/// `exec`：程序须存在（Hub 直接运行它，4.3）。
fn app_exec(i: &AppInput) -> Option<Finding> {
    if i.reg.activation.kind != kinds::EXEC {
        return None;
    }
    let idle = i.idle();
    let Some(program) = exec_program(i.reg) else {
        return Some(Finding::new(Level::Error, format!("{idle}，激活方式 exec 既没有 target 也没有 executable")).hint(REINSTALL_HINT));
    };
    Some(match missing_file(program) {
        Some(why) => Finding::new(Level::Error, format!("{idle}，激活程序 {program} {why}（调用报 APP_NOT_INSTALLED）")).hint(REINSTALL_HINT),
        None => Finding::new(Level::Ok, format!("{idle}；调用时运行 {program} 激活")),
    })
}

/// `uri` / `aumid`：交给系统激活，须有 `target`。
fn app_system_activation(i: &AppInput) -> Option<Finding> {
    let kind = i.reg.activation.kind.as_str();
    if kind != kinds::URI && kind != kinds::AUMID {
        return None;
    }
    let idle = i.idle();
    let target = i.reg.activation.target.as_str();
    Some(if target.is_empty() {
        Finding::new(Level::Error, format!("{idle}，激活方式 {kind} 缺少 target")).hint(REINSTALL_HINT)
    } else {
        Finding::new(Level::Ok, format!("{idle}；调用时经 {kind} 激活（{target}）"))
    })
}

/// 兜底：`none`、`dbus` 或未知的激活方式。
fn app_not_activatable(i: &AppInput) -> Option<Finding> {
    Some(Finding::new(
        Level::Info,
        format!("{}，激活方式「{}」不能由 Hub 执行：只在 App 运行时可调用", i.idle(), i.reg.activation.kind),
    ))
}

/// 汇总为一项检查。
pub fn evaluate(p: &PipeProbe) -> Check {
    let sid = match &p.sid {
        Ok(s) => s.as_str(),
        Err(e) => {
            return Check::new(ID, TITLE, Level::Error, format!("无法取得当前用户 SID：{e}（Hub 的管道连接器同样无法启动）"))
                .details(json!({ "sidError": e }));
        }
    };
    let classified: Option<Vec<(&str, Listed)>> =
        p.listed.as_ref().ok().map(|names| names.iter().map(|n| (n.as_str(), classify(n, sid))).collect());
    let all = classified.as_deref().unwrap_or_default();
    let registered = |id: &str| p.registrations.iter().any(|r| r.app_id == id);

    let mut findings: Vec<Finding> = Vec::new();
    if let Err(e) = &p.listed {
        findings.push(Finding::new(Level::Warn, format!("无法列出 {PIPE_DIR}（{e}）：App 是否在运行未知")));
    }

    let mut apps = Vec::new();
    for reg in &p.registrations {
        let Ok(address) = Address::new(&reg.app_id, None) else { continue };
        let pipe = names::pipe_name(sid, &address);
        let of_app = all.iter().filter(|(_, l)| matches!(l, Listed::Mine { app_id, .. } if *app_id == reg.app_id));
        let input = AppInput {
            reg,
            running: classified.as_ref().map(|_| of_app.clone().any(|(_, l)| matches!(l, Listed::Mine { instance: None, .. }))),
            instances: of_app.filter(|(_, l)| matches!(l, Listed::Mine { instance: Some(_), .. })).count(),
        };
        if let Some(f) = APP_RULES.iter().find_map(|rule| rule(&input)) {
            apps.push(json!({
                "appId": reg.app_id, "pipe": pipe, "running": input.running, "instances": input.instances,
                "activation": reg.activation.kind, "level": f.level,
            }));
            findings.push(Finding { text: format!("{}：{}", reg.app_id, f.text), ..f });
        }
    }

    let unregistered: Vec<&str> =
        all.iter().filter(|(_, l)| matches!(l, Listed::Mine { app_id, .. } if !registered(app_id))).map(|(n, _)| *n).collect();
    for name in &unregistered {
        findings.push(
            Finding::new(Level::Warn, format!("管道 {name} 没有对应的登记文件：Hub 发现不了这个 App")).hint(UNREGISTERED_HINT),
        );
    }
    let foreign: Vec<&str> = all.iter().filter(|(_, l)| matches!(l, Listed::Foreign { .. })).map(|(n, _)| *n).collect();
    let mut foreign_sids: Vec<&str> =
        all.iter().filter_map(|(_, l)| if let Listed::Foreign { sid } = l { Some(sid.as_str()) } else { None }).collect();
    foreign_sids.sort_unstable();
    foreign_sids.dedup();
    if !foreign.is_empty() {
        findings.push(Finding::new(
            Level::Info,
            format!(
                "另有 {} 条 appmcp- 管道属于其他用户（SID {}）：Hub 只打开当前用户 SID 的管道",
                foreign.len(),
                foreign_sids.join("、")
            ),
        ));
    }
    let malformed: Vec<&str> = all.iter().filter(|(_, l)| *l == Listed::Malformed).map(|(n, _)| *n).collect();
    if !malformed.is_empty() {
        findings.push(Finding::new(
            Level::Info,
            format!("{} 条 appmcp- 管道不符合命名规则（spec/naming.md 4.3），Hub 不会使用：{}", malformed.len(), malformed.join("、")),
        ));
    }

    let details = json!({
        "sid": sid,
        "ownerVerified": false,
        "listError": p.listed.as_ref().err(),
        "apps": apps,
        "unregistered": unregistered,
        "foreign": foreign,
        "malformed": malformed,
    });
    if findings.is_empty() {
        return Check::new(ID, TITLE, Level::Info, "没有 App 登记文件，也没有当前用户的 appmcp- 管道")
            .hint("需要按名寻址的未打包 App 用 app-mcp-host app install 登记（spec/naming.md 5.3）")
            .details(details);
    }
    let level = worst(findings.iter().filter(|f| f.level != Level::Info), Level::Info);
    let summary = findings.iter().map(|f| f.text.as_str()).collect::<Vec<_>>().join("；");
    let mut c = Check::new(ID, TITLE, level, summary).details(details);
    c.hint = joined_hints(&findings);
    c
}

/// 列出 `dir` 中以 `appmcp-` 开头的名字（最多读 [`MAX_PIPE_ENTRIES`] 项）。
///
/// @why 用目录查询（`FindFirstFile` / `FindNextFile`）而不是逐个打开：打开管道会被 App 当作通道接受。
fn list_pipes(dir: &Path) -> Result<Vec<String>, String> {
    let prefix = listed_prefix();
    let entries = std::fs::read_dir(dir).map_err(|e| e.to_string())?;
    Ok(entries
        .take(MAX_PIPE_ENTRIES)
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.get(..prefix.len()).is_some_and(|head| head.eq_ignore_ascii_case(prefix)))
        .collect())
}

/// [`list_pipes`] 加超时（B-08）：在阻塞线程上运行，超时即返回（该线程受条数上限约束，随后自行结束）。
async fn list_bounded(dir: PathBuf, timeout: Duration) -> Result<Vec<String>, String> {
    let task = tokio::task::spawn_blocking(move || list_pipes(&dir));
    match tokio::time::timeout(timeout, task).await {
        Ok(Ok(listed)) => listed,
        Ok(Err(e)) => Err(format!("列出管道的任务异常：{e}")),
        Err(_) => Err(format!("{} 秒内没有列完", timeout.as_secs())),
    }
}

#[cfg(windows)]
fn current_sid() -> Result<String, String> {
    app_mcp_protocol::endpoint::win::current_user_sid().map_err(|e| e.to_string())
}

#[cfg(not(windows))]
fn current_sid() -> Result<String, String> {
    Err("当前平台没有 Windows 用户 SID".to_owned())
}

/// 运行检查。
pub async fn check(env: &NamingEnv) -> Check {
    let registrations = registration::scan(&env.registration_dirs, Platform::current(), &[])
        .into_iter()
        .filter_map(|r| r.registration)
        .collect();
    let probe = PipeProbe { sid: current_sid(), listed: list_bounded(PathBuf::from(PIPE_DIR), env.timeout).await, registrations };
    evaluate(&probe)
}

#[cfg(test)]
mod tests {
    use super::*;
    use app_mcp_protocol::naming::registration::Activation;

    const SID: &str = "S-1-5-21-1-2-3-1001";

    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            let d = std::env::temp_dir().join(format!("app-mcp-doctor-pipes-{:032x}", rand::random::<u128>()));
            std::fs::create_dir_all(&d).unwrap();
            Self(d)
        }
        /// 一个存在的"程序"文件（绝对路径）。
        fn program(&self, name: &str) -> String {
            let p = self.0.join(name);
            std::fs::write(&p, b"x").unwrap();
            p.to_string_lossy().into_owned()
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn reg(app_id: &str, executable: Option<&str>, kind: &str, target: &str) -> Registration {
        Registration {
            registration_version: 1,
            app_id: app_id.to_owned(),
            name: String::new(),
            source: "manual".to_owned(),
            manifest: None,
            manifest_sha256: None,
            executable: executable.map(str::to_owned),
            activation: Activation { kind: kind.to_owned(), target: target.to_owned() },
        }
    }

    /// 目录项中的名字（不含 `\\.\pipe\`）。
    fn listed(sid: &str, app: &str) -> String {
        let full = names::pipe_name(sid, &Address::parse(&format!("appmcp://{app}")).unwrap());
        full.strip_prefix(PIPE_DIR).unwrap().to_owned()
    }

    fn probe(listed: Result<Vec<String>, String>, registrations: Vec<Registration>) -> PipeProbe {
        PipeProbe { sid: Ok(SID.to_owned()), listed, registrations }
    }

    #[test]
    fn prefix_is_derived_from_protocol() {
        assert_eq!(listed_prefix(), "appmcp-");
    }

    #[test]
    fn classify_names() {
        let mine = |a: &str, i: Option<&str>| Listed::Mine { app_id: a.to_owned(), instance: i.map(str::to_owned) };
        assert_eq!(classify(&listed(SID, "my-shop"), SID), mine("my-shop", None));
        assert_eq!(classify(&listed(SID, "my-shop/w-2"), SID), mine("my-shop", Some("w-2")));
        assert_eq!(classify(&listed(SID, "my-shop").to_uppercase(), SID), mine("my-shop", None), "管道名不区分大小写");
        assert_eq!(classify(&listed("S-1-5-21-9-9-9-500", "shop"), SID), Listed::Foreign { sid: "S-1-5-21-9-9-9-500".to_owned() });
        for bad in [format!("appmcp-{SID}-Bad_Id"), format!("appmcp-{SID}-shop.default"), "appmcp-x".to_owned(), "appmcp-s-1-shop".to_owned(), "other".to_owned()] {
            assert_eq!(classify(&bad, SID), Listed::Malformed, "{bad}");
        }
    }

    /// 规则表的每一行与兜底各命中一次（T-09）。
    #[test]
    fn each_app_rule() {
        let s = Scratch::new();
        let exe = s.program("shop.exe");
        let gone = s.0.join("gone.exe").to_string_lossy().into_owned();
        /// （名称，登记，管道是否存在，实例管道数，期望级别，期望说明片段）
        type Case<'a> = (&'a str, Registration, Option<bool>, usize, Level, String);
        let cases: Vec<Case> = vec![
            ("ignored", reg("a", Some(&gone), kinds::EXEC, ""), Some(true), 0, Level::Error, format!("登记的程序 {gone} 不存在")),
            ("running", reg("a", Some(&exe), kinds::NONE, ""), Some(true), 2, Level::Ok, "运行中（管道存在，另有 2 个实例管道）".into()),
            ("exec ok", reg("a", Some(&exe), kinds::EXEC, ""), Some(false), 0, Level::Ok, format!("未运行；调用时运行 {exe} 激活")),
            ("exec target gone", reg("a", Some(&exe), kinds::EXEC, &gone), Some(false), 0, Level::Error, format!("激活程序 {gone} 不存在")),
            ("exec relative", reg("a", None, kinds::EXEC, "shop.exe"), None, 0, Level::Error, "运行状态未知，激活程序 shop.exe 不是绝对路径".into()),
            ("exec nothing", reg("a", None, kinds::EXEC, ""), Some(false), 0, Level::Error, "既没有 target 也没有 executable".into()),
            ("uri", reg("a", None, kinds::URI, "appmcp-shop"), Some(false), 0, Level::Ok, "未运行；调用时经 uri 激活（appmcp-shop）".into()),
            ("aumid no target", reg("a", None, kinds::AUMID, ""), Some(false), 0, Level::Error, "激活方式 aumid 缺少 target".into()),
            ("none", reg("a", None, kinds::NONE, ""), Some(false), 0, Level::Info, "激活方式「none」不能由 Hub 执行".into()),
            ("dbus", reg("a", None, kinds::DBUS, "x"), Some(false), 0, Level::Info, "激活方式「dbus」不能由 Hub 执行".into()),
        ];
        for (name, reg, running, instances, level, needle) in cases {
            let input = AppInput { reg: &reg, running, instances };
            let f = APP_RULES.iter().find_map(|rule| rule(&input)).unwrap_or_else(|| panic!("{name}：没有规则命中"));
            assert!(f.level == level && f.text.contains(&needle), "{name}: {f:?}");
        }
    }

    #[test]
    fn evaluate_maps_registrations_to_pipes() {
        let s = Scratch::new();
        let exe = s.program("shop.exe");
        let gone = s.0.join("gone.exe").to_string_lossy().into_owned();
        let listed_names = vec![
            listed(SID, "my-shop"),
            listed(SID, "my-shop/w2"),
            listed(SID, "stray"),
            listed("S-1-5-21-7-7-7-1002", "my-shop"),
            "appmcp-junk".to_owned(),
        ];
        let regs = vec![reg("my-shop", Some(&exe), kinds::EXEC, ""), reg("notes", Some(&exe), kinds::EXEC, &gone)];
        let c = evaluate(&probe(Ok(listed_names), regs));
        assert_eq!(c.status, Level::Error, "{}", c.summary);
        for needle in [
            "my-shop：运行中（管道存在，另有 1 个实例管道）",
            &format!("notes：未运行，激活程序 {gone} 不存在"),
            &format!("管道 appmcp-{SID}-stray 没有对应的登记文件"),
            "另有 1 条 appmcp- 管道属于其他用户（SID S-1-5-21-7-7-7-1002）",
            "1 条 appmcp- 管道不符合命名规则",
        ] {
            assert!(c.summary.contains(needle), "缺少「{needle}」：{}", c.summary);
        }
        assert!(c.hint.as_deref().is_some_and(|h| h.contains("app install")), "{:?}", c.hint);
        assert_eq!(c.details["apps"][0]["pipe"], format!(r"\\.\pipe\appmcp-{SID}-my-shop"));
        assert_eq!(c.details["apps"][1]["running"], false);
        assert_eq!(c.details["unregistered"].as_array().map(Vec::len), Some(1));
        assert_eq!(c.details["ownerVerified"], false);
    }

    #[test]
    fn evaluate_edge_cases() {
        let s = Scratch::new();
        let exe = s.program("shop.exe");
        // 全部正常：级别为 ok。
        let c = evaluate(&probe(Ok(vec![]), vec![reg("my-shop", Some(&exe), kinds::EXEC, "")]));
        assert_eq!((c.status, c.summary.as_str()), (Level::Ok, format!("my-shop：未运行；调用时运行 {exe} 激活").as_str()));
        // 列不出管道：注意，运行状态未知。
        let c = evaluate(&probe(Err("拒绝访问".into()), vec![reg("my-shop", Some(&exe), kinds::EXEC, "")]));
        assert_eq!(c.status, Level::Warn);
        assert!(c.summary.contains("无法列出") && c.summary.contains("运行状态未知"), "{}", c.summary);
        // 什么都没有：信息。
        let c = evaluate(&probe(Ok(vec![]), vec![]));
        assert_eq!(c.status, Level::Info);
        // 只有不可激活的登记：信息（不是正常，也不是问题）。
        let c = evaluate(&probe(Ok(vec![]), vec![reg("my-shop", None, kinds::NONE, "")]));
        assert_eq!(c.status, Level::Info, "{}", c.summary);
        // 取不到 SID：错误。
        let c = evaluate(&PipeProbe { sid: Err("x".into()), listed: Ok(vec![]), registrations: vec![] });
        assert_eq!(c.status, Level::Error);
    }

    #[tokio::test]
    async fn list_pipes_filters_prefix_and_reports_errors() {
        let s = Scratch::new();
        for name in [listed(SID, "my-shop"), "APPMCP-upper".to_owned(), "app-mcp-host-pipe".to_owned(), "other".to_owned()] {
            std::fs::write(s.0.join(name), b"").unwrap();
        }
        let mut got = list_bounded(s.0.clone(), Duration::from_secs(5)).await.unwrap();
        got.sort();
        assert_eq!(got, ["APPMCP-upper".to_owned(), listed(SID, "my-shop")]);
        assert!(list_bounded(s.0.join("missing"), Duration::from_secs(5)).await.is_err());
    }
}
