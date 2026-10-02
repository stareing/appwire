//! `naming.android`：经 adb 检查已连接设备上的按名寻址声明（spec/naming.md 4.2、第 12 节 `ACTIVATION_BLOCKED`）。
//!
//! 只在 PATH 中有 adb 且有已连接设备时运行；每条 adb 命令有超时。只读：查询 `dev.appmcp.TOOLS` / `dev.appmcp.HUB` 的 Service 声明、
//! 系统属性（识别 ROM）与 `ActivityManager` 警告日志（系统拦截绑定的记录）；不绑定、不启动任何 App、不改系统设置。

use std::collections::BTreeMap;
use std::path::Path;

use app_mcp_protocol::naming::codes;
use serde::Serialize;
use serde_json::json;

use super::{Finding, NamingEnv, joined_hints, with_naming_code, worst};
use crate::doctor::command::{ToolFailure, run_tool};
use crate::doctor::{Check, Level};

pub const ID: &str = "naming.android";
const TITLE: &str = "名字服务：Android";

/// App 端 Service 的 Intent 动作（spec/naming.md 4.2）。
const TOOLS_ACTION: &str = "dev.appmcp.TOOLS";
/// 独立 Hub App 的 Intent 动作（TASKS 4g d）。
const HUB_ACTION: &str = "dev.appmcp.HUB";
/// 最多检查的设备数（B-07）。
const MAX_DEVICES: usize = 4;
/// 日志只看末尾这么多字节（B-07）。
const MAX_LOG_BYTES: usize = 1024 * 1024;

/// `cmd package query-services` 中的一个 Service 声明。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceDecl {
    pub package: String,
    pub class: String,
    pub enabled: Option<bool>,
    pub exported: Option<bool>,
    /// 绑定所需的权限（`null` 时为 `None`）。
    pub permission: Option<String>,
    /// 应用名（`nonLocalizedLabel`，可能没有）。
    pub label: Option<String>,
}

impl ServiceDecl {
    fn component(&self) -> String {
        format!("{}/{}", self.package, self.class)
    }
    fn display(&self) -> String {
        match &self.label {
            Some(l) => format!("{}「{l}」", self.component()),
            None => self.component(),
        }
    }
}

/// 解析 `adb shell cmd package query-services -a <动作>` 的输出。
///
/// 输出形如 `1 services found:` 后接每个 `Service #N:` 块，块内 `ServiceInfo:` 与 `ApplicationInfo:` 之间为 Service 自己的字段
/// （`name=`、`packageName=`、`enabled=… exported=…`、`permission=`）；`ApplicationInfo:` 之后的同名字段属于应用，不取。
/// 不认识的输出（旧系统没有该命令）为错误。
pub fn parse_query_services(text: &str) -> Result<Vec<ServiceDecl>, String> {
    if text.contains("No services found") {
        return Ok(Vec::new());
    }
    if !text.contains("services found") {
        return Err(first_line(text));
    }
    let mut out = Vec::new();
    for block in text.split("Service #").skip(1) {
        let mut decl = ServiceDecl::default();
        let mut section = "";
        for line in block.lines().map(str::trim) {
            match line {
                "ServiceInfo:" | "ApplicationInfo:" => {
                    section = line;
                    continue;
                }
                _ => {}
            }
            if section == "ApplicationInfo:" {
                if let Some(rest) = line.split_once("nonLocalizedLabel=").map(|(_, r)| r) {
                    let label = rest.split_once(" icon=").map_or(rest, |(l, _)| l).trim();
                    decl.label = (!label.is_empty() && label != "null").then(|| label.to_owned());
                }
                continue;
            }
            if section != "ServiceInfo:" {
                continue;
            }
            for (k, v) in line.split_whitespace().filter_map(|kv| kv.split_once('=')) {
                match k {
                    "name" => decl.class = v.to_owned(),
                    "packageName" => decl.package = v.to_owned(),
                    "enabled" => decl.enabled = v.parse().ok(),
                    "exported" => decl.exported = v.parse().ok(),
                    "permission" => decl.permission = (v != "null").then(|| v.to_owned()),
                    _ => {}
                }
            }
        }
        if !decl.package.is_empty() && !decl.class.is_empty() {
            out.push(decl);
        }
    }
    Ok(out)
}

fn first_line(text: &str) -> String {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("（无输出）");
    line.chars().take(200).collect()
}

/// 解析 `adb devices`：`(序列号, 状态)`。
pub fn parse_devices(text: &str) -> Vec<(String, String)> {
    text.lines()
        .skip_while(|l| !l.starts_with("List of devices"))
        .skip(1)
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            Some((f.next()?.to_owned(), f.next()?.to_owned()))
        })
        .collect()
}

/// 解析 `adb shell getprop`：`[key]: [value]`。
pub fn parse_getprop(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|l| {
            let (k, v) = l.trim().split_once("]: [")?;
            Some((k.strip_prefix('[')?.to_owned(), v.strip_suffix(']')?.to_owned()))
        })
        .collect()
}

/// 有关联启动 / 自启动管控的 ROM。
pub struct Rom {
    pub name: &'static str,
    /// 任一属性存在且非空即认定。
    props: &'static [&'static str],
    /// 放行路径。
    pub settings: &'static str,
    /// 拦截行为已在真机确认。
    pub verified: bool,
}

/// ROM 识别表（按顺序第一条命中）。只有 Flyme 经过真机确认（TASKS 4d 真机结果），其余为厂商常见设置位置，路径因版本而异。
pub const ROMS: &[Rom] = &[
    Rom {
        name: "Flyme",
        props: &["ro.build.flyme.version", "ro.flyme.version.id"],
        settings: "设置 → 应用管理 → 该 App → 耗电和后台（二级页）中允许自启动与关联启动；放行后约 1 分钟生效",
        verified: true,
    },
    Rom {
        name: "MIUI / HyperOS",
        props: &["ro.miui.ui.version.name", "ro.mi.os.version.name"],
        settings: "设置 → 应用设置 → 应用管理 → 该 App → 自启动；省电策略设为无限制",
        verified: false,
    },
    Rom {
        name: "EMUI / HarmonyOS",
        props: &["ro.build.version.emui", "hw_sc.build.platform.version"],
        settings: "设置 → 应用和服务 → 应用启动管理 → 该 App 改为手动管理并允许自启动、关联启动",
        verified: false,
    },
    Rom {
        name: "ColorOS",
        props: &["ro.build.version.oplusrom", "ro.build.version.opporom"],
        settings: "设置 → 应用 → 自启动管理 / 关联启动中允许该 App",
        verified: false,
    },
    Rom {
        name: "OriginOS / Funtouch OS",
        props: &["ro.vivo.os.version", "ro.vivo.os.build.display.id"],
        settings: "i管家 / 设置 → 应用与权限 → 权限管理 → 自启动、关联启动中允许该 App",
        verified: false,
    },
];

pub fn rom_of(props: &BTreeMap<String, String>) -> Option<&'static Rom> {
    ROMS.iter().find(|r| r.props.iter().any(|p| props.get(*p).is_some_and(|v| !v.is_empty())))
}

/// 系统拦截绑定的日志特征：`(特征文本, 来源)`。只收录真机见过的原文。
const BLOCK_PATTERNS: &[(&str, &str)] = &[
    // Flyme（魅族 18 Pro / Android 13，2026-10-02）：`W ActivityManager: u0 Binding to a service in package <包名> requires a ifw permit[3rd app inter-call]`
    ("requires a ifw permit", "Flyme"),
];

/// 一条拦截记录：包名与最近一次出现的日志时间（`MM-DD hh:mm:ss.mmm`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Blocked {
    pub package: String,
    pub last_seen: String,
}

/// 日志中被系统拦截绑定的包（按首次出现排序，时间取最近一次）。
pub fn blocked_packages(log: &str) -> Vec<Blocked> {
    let mut out: Vec<Blocked> = Vec::new();
    for line in log.lines() {
        if !BLOCK_PATTERNS.iter().any(|(needle, _)| line.contains(needle)) {
            continue;
        }
        let Some(pkg) = line.split_once("package ").and_then(|(_, r)| r.split_whitespace().next()) else { continue };
        let when = line.split_whitespace().take(2).collect::<Vec<_>>().join(" ");
        match out.iter_mut().find(|b| b.package == pkg) {
            Some(b) => b.last_seen = when,
            None => out.push(Blocked { package: pkg.to_owned(), last_seen: when }),
        }
    }
    out
}

/// 一台设备的探测结果。
#[derive(Debug)]
pub struct DeviceProbe {
    pub serial: String,
    pub tools: Result<Vec<ServiceDecl>, String>,
    pub hub: Result<Vec<ServiceDecl>, String>,
    pub props: BTreeMap<String, String>,
    /// `ActivityManager` 警告日志；读取失败时为 `Err`。
    pub log: Result<String, String>,
}

/// Service 声明规则表（`(适用于 Hub App, 规则)`）。
type DeclRule = fn(&ServiceDecl) -> Option<Finding>;
const DECL_RULES: &[(bool, DeclRule)] = &[(true, rule_exported), (false, rule_enabled), (false, rule_permission)];

fn rule_exported(d: &ServiceDecl) -> Option<Finding> {
    (d.exported == Some(false)).then(|| {
        Finding::new(Level::Error, format!("{} 未导出：其他 App 无法绑定（BIND_PERMISSION_DENIED）", d.component()))
            .hint("去掉清单中把该 Service 设为 android:exported=\"false\" 的覆盖（SDK 的清单合并默认导出）")
    })
}

fn rule_enabled(d: &ServiceDecl) -> Option<Finding> {
    (d.enabled == Some(false)).then(|| {
        Finding::new(Level::Warn, format!("{} 已被禁用：Hub 发现不到", d.component()))
            .hint("adb shell pm enable <包名>/<类名>，或检查 App 是否在运行时禁用了该组件")
    })
}

fn rule_permission(d: &ServiceDecl) -> Option<Finding> {
    let p = d.permission.as_deref()?;
    Some(
        Finding::new(Level::Warn, format!("{} 要求权限 {p}：Hub 未持有时绑定被拒（BIND_PERMISSION_DENIED）", d.component()))
            .hint("按名寻址不以权限授权（spec/naming.md 4.2，由 open() 内校验调用方）：去掉该 Service 的 android:permission"),
    )
}

/// 日志末尾至多 [`MAX_LOG_BYTES`] 字节（从字符边界开始）。
fn log_tail(log: &str) -> &str {
    let mut start = log.len().saturating_sub(MAX_LOG_BYTES);
    while !log.is_char_boundary(start) {
        start += 1;
    }
    &log[start..]
}

/// 一台设备的发现与摘要行。
fn evaluate_device(d: &DeviceProbe) -> (Vec<Finding>, String, bool) {
    let rom = rom_of(&d.props);
    let model = d.props.get("ro.product.model").map(String::as_str).unwrap_or("未知型号");
    let rom_text = rom.map(|r| format!("，{}", r.name)).unwrap_or_default();
    let mut findings = Vec::new();
    let mut parts = Vec::new();

    let tools: &[ServiceDecl] = match &d.tools {
        Ok(t) => t,
        Err(e) => {
            findings.push(Finding::new(Level::Info, format!("{}：查询 {TOOLS_ACTION} 失败：{e}", d.serial)));
            &[]
        }
    };
    let hub: &[ServiceDecl] = match &d.hub {
        Ok(h) => h,
        Err(e) => {
            findings.push(Finding::new(Level::Info, format!("{}：查询 {HUB_ACTION} 失败：{e}", d.serial)));
            &[]
        }
    };
    parts.push(if tools.is_empty() {
        format!("没有声明 {TOOLS_ACTION} 的 App")
    } else {
        format!("{TOOLS_ACTION}：{}", tools.iter().map(ServiceDecl::display).collect::<Vec<_>>().join("、"))
    });
    parts.push(match hub.first() {
        Some(h) => format!("独立 Hub App：{}", h.display()),
        None => format!("没有独立 Hub App（{HUB_ACTION}）；PC 端 Host 经 adb reverse 的路径不受影响"),
    });
    for (decl, is_hub) in tools.iter().map(|t| (t, false)).chain(hub.iter().map(|h| (h, true))) {
        findings.extend(DECL_RULES.iter().filter(|(for_hub, _)| !is_hub || *for_hub).filter_map(|(_, rule)| rule(decl)));
    }

    let relevant: Vec<&str> = tools.iter().chain(hub).map(|s| s.package.as_str()).collect();
    let mut blocked = false;
    if let Ok(log) = &d.log {
        let hits: Vec<String> = blocked_packages(log_tail(log))
            .into_iter()
            .filter(|b| relevant.contains(&b.package.as_str()))
            .map(|b| format!("{}（最近 {}）", b.package, b.last_seen))
            .collect();
        if !hits.is_empty() {
            blocked = true;
            let settings = rom.map(|r| r.settings).unwrap_or("系统设置中该 App 的自启动 / 关联启动开关");
            findings.push(
                Finding::new(
                    Level::Warn,
                    format!(
                        "{}：系统日志记录了对 {} 的服务绑定被拦截（调用返回 USER_ACTION_REQUIRED / os-permission；记录可能早于放行设置，放行后以新的调用结果为准）",
                        d.serial,
                        hits.join("、")
                    ),
                )
                .hint(format!("由机主在系统设置中放行（{settings}）；Hub App 本身被拦时同样放行 Hub App。doctor 不修改设置")),
            );
        }
    }
    let declared = !(tools.is_empty() && hub.is_empty());
    if let Some(r) = rom.filter(|_| declared && !blocked) {
        let text = if r.verified {
            format!("{} 默认拦截第三方 App 之间的服务绑定（已在真机确认）", r.name)
        } else {
            format!("{} 可能限制 App 之间的关联启动（未经真机验证）", r.name)
        };
        findings.push(Finding::new(Level::Info, format!("{}：{text}", d.serial)).hint(format!("调用报 ACTIVATION_BLOCKED / os-permission 时：{}", r.settings)));
    }
    (findings, format!("{}（{model}{rom_text}）：{}", d.serial, parts.join("；")), blocked)
}

/// 设备列表或 adb 本身的状态。
pub enum Devices {
    /// adb 不在 PATH。
    NoAdb,
    /// `adb devices` 失败。
    Failed(String),
    /// 已就绪的设备探测结果，以及未就绪的设备 `(序列号, 状态)`。
    Probed { ready: Vec<DeviceProbe>, not_ready: Vec<(String, String)> },
}

/// 汇总为一项检查。
pub fn evaluate(devices: &Devices) -> Check {
    let (ready, not_ready) = match devices {
        Devices::NoAdb => return Check::new(ID, TITLE, Level::Skip, "PATH 中没有 adb"),
        Devices::Failed(e) => {
            return Check::new(ID, TITLE, Level::Skip, format!("adb devices 失败：{e}")).hint("adb start-server 后重试");
        }
        Devices::Probed { ready, not_ready } => (ready, not_ready),
    };
    let not_ready_text: Vec<String> = not_ready.iter().map(|(s, st)| format!("{s}（{st}）")).collect();
    if ready.is_empty() {
        let summary = if not_ready.is_empty() {
            "没有已连接的 Android 设备".to_owned()
        } else {
            format!("设备未就绪：{}", not_ready_text.join("、"))
        };
        return Check::new(ID, TITLE, Level::Skip, summary).hint("unauthorized：在手机上允许 USB 调试；offline：adb reconnect");
    }
    let mut findings = Vec::new();
    let mut lines = Vec::new();
    let mut blocked = false;
    let mut any_decl = false;
    for d in ready {
        let (f, line, b) = evaluate_device(d);
        findings.extend(f);
        lines.push(line);
        blocked |= b;
        any_decl |= d.tools.as_ref().is_ok_and(|t| !t.is_empty()) || d.hub.as_ref().is_ok_and(|h| !h.is_empty());
    }
    if !not_ready.is_empty() {
        lines.push(format!("未就绪：{}", not_ready_text.join("、")));
    }
    let fallback = if any_decl { Level::Ok } else { Level::Info };
    let level = worst(findings.iter().filter(|f| f.level != Level::Info), fallback);
    lines.extend(findings.iter().map(|f| f.text.clone()));
    let details = json!({
        "devices": ready.iter().map(|d| json!({
            "serial": d.serial,
            "model": d.props.get("ro.product.model"),
            "sdk": d.props.get("ro.build.version.sdk"),
            "rom": rom_of(&d.props).map(|r| r.name),
            "tools": d.tools.as_ref().ok(),
            "hub": d.hub.as_ref().ok(),
            "errors": [d.tools.as_ref().err(), d.hub.as_ref().err(), d.log.as_ref().err()],
        })).collect::<Vec<_>>(),
        "notReady": not_ready,
        "findings": findings,
    });
    let mut c = Check::new(ID, TITLE, level, lines.join("；")).details(details);
    c.hint = joined_hints(&findings);
    if blocked {
        c = with_naming_code(c, codes::ACTIVATION_BLOCKED);
    }
    c
}

fn failure_text(e: ToolFailure) -> String {
    e.to_string()
}

/// 在一台设备上运行 adb 子命令。
async fn adb_on(adb: &Path, serial: &str, args: &[&str], env: &NamingEnv) -> Result<String, String> {
    let mut full = vec!["-s", serial];
    full.extend_from_slice(args);
    let out = run_tool(adb, &full, env.timeout).await.map_err(failure_text)?;
    if out.success { Ok(out.stdout) } else { Err(first_line(&format!("{}\n{}", out.stderr, out.stdout))) }
}

async fn probe_device(adb: &Path, serial: String, env: &NamingEnv) -> DeviceProbe {
    let query = |action: &'static str| {
        let serial = serial.as_str();
        async move {
            adb_on(adb, serial, &["shell", "cmd", "package", "query-services", "-a", action], env)
                .await
                .and_then(|t| parse_query_services(&t))
        }
    };
    let (tools, hub, props, log) = tokio::join!(
        query(TOOLS_ACTION),
        query(HUB_ACTION),
        adb_on(adb, &serial, &["shell", "getprop"], env),
        adb_on(adb, &serial, &["logcat", "-d", "-s", "ActivityManager:W"], env),
    );
    DeviceProbe { props: props.map(|t| parse_getprop(&t)).unwrap_or_default(), tools, hub, log, serial }
}

/// 运行检查。
pub async fn check(env: &NamingEnv) -> Check {
    let Some(adb) = env.adb.as_deref() else {
        return evaluate(&Devices::NoAdb);
    };
    let devices = match run_tool(adb, &["devices"], env.timeout).await {
        Ok(o) if o.success => parse_devices(&o.stdout),
        Ok(o) => return evaluate(&Devices::Failed(first_line(&o.stderr))),
        Err(e) => return evaluate(&Devices::Failed(failure_text(e))),
    };
    let (ready, not_ready): (Vec<_>, Vec<_>) = devices.into_iter().partition(|(_, st)| st == "device");
    let mut probes = Vec::new();
    for (serial, _) in ready.into_iter().take(MAX_DEVICES) {
        probes.push(probe_device(adb, serial, env).await);
    }
    evaluate(&Devices::Probed { ready: probes, not_ready })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 魅族 18 Pro（Flyme 10，Android 13）上 `cmd package query-services -a dev.appmcp.TOOLS` 的实际输出（节选 ApplicationInfo）。
    const TOOLS_OUTPUT: &str = "1 services found:
  Service #0:
    priority=0 preferredOrder=0 match=0x108000 specificIndex=-1 isDefault=false
    ServiceInfo:
      name=dev.appmcp.android.ToolsService
      packageName=dev.appmcp.sample.android
      enabled=true exported=true directBootAware=false
      permission=null
      flags=0x0
      ApplicationInfo:
        name=dev.appmcp.sample.android.SampleApp
        packageName=dev.appmcp.sample.android
        labelRes=0x0 nonLocalizedLabel=app-mcp 示例 icon=0x0 banner=0x0
        enabled=true minSdkVersion=24 targetSdkVersion=35 versionCode=1 targetSandboxVersion=1
";

    const HUB_OUTPUT: &str = "1 services found:
  Service #0:
    priority=0 preferredOrder=0 match=0x108000 specificIndex=-1 isDefault=false
    ServiceInfo:
      name=dev.appmcp.hubapp.HubService
      packageName=dev.appmcp.hub.app
      enabled=true exported=true directBootAware=false
      permission=null
";

    /// 同一设备上的实际日志行。
    const FLYME_BLOCK_LOG: &str = "10-02 17:45:35.977  1576  3940 W ActivityManager: u0 Binding to a service in package dev.appmcp.sample.android requires a ifw permit[3rd app inter-call]\n\
10-02 17:45:36.001  1576  3940 W ActivityManager: u0 Binding to a service in package com.unrelated.app requires a ifw permit[3rd app inter-call]\n";

    fn sample() -> ServiceDecl {
        ServiceDecl {
            package: "dev.appmcp.sample.android".into(),
            class: "dev.appmcp.android.ToolsService".into(),
            enabled: Some(true),
            exported: Some(true),
            permission: None,
            label: Some("app-mcp 示例".into()),
        }
    }

    fn flyme_props() -> BTreeMap<String, String> {
        parse_getprop("[ro.build.flyme.version]: [10]\n[ro.product.model]: [MEIZU 18 Pro]\n[ro.build.version.sdk]: [33]\n")
    }

    fn device(tools: Vec<ServiceDecl>, log: &str, props: BTreeMap<String, String>) -> DeviceProbe {
        DeviceProbe {
            serial: "191QNEATV2S72".into(),
            tools: Ok(tools),
            hub: parse_query_services(HUB_OUTPUT),
            props,
            log: Ok(log.to_owned()),
        }
    }

    #[test]
    fn parses_real_query_services_output() {
        assert_eq!(parse_query_services(TOOLS_OUTPUT).unwrap(), vec![sample()], "ApplicationInfo 中的同名字段不覆盖 Service 的");
        let hub = parse_query_services(HUB_OUTPUT).unwrap();
        assert_eq!(hub[0].component(), "dev.appmcp.hub.app/dev.appmcp.hubapp.HubService");
        assert_eq!(hub[0].label, None);
        assert_eq!(parse_query_services("No services found\n").unwrap(), vec![]);
        assert!(parse_query_services("Unknown command: query-services\n").unwrap_err().contains("Unknown command"));
        let with_perm = TOOLS_OUTPUT.replace("permission=null", "permission=dev.appmcp.permission.BIND_TOOLS").replace("exported=true", "exported=false");
        let d = &parse_query_services(&with_perm).unwrap()[0];
        assert_eq!((d.exported, d.permission.as_deref()), (Some(false), Some("dev.appmcp.permission.BIND_TOOLS")));
    }

    #[test]
    fn parses_devices_and_props() {
        let text = "* daemon started successfully\nList of devices attached\n191QNEATV2S72          device product:meizu_18Pro_CN model:MEIZU_18_Pro\nemulator-5554\tunauthorized\n\n";
        assert_eq!(
            parse_devices(text),
            [("191QNEATV2S72".to_owned(), "device".to_owned()), ("emulator-5554".to_owned(), "unauthorized".to_owned())]
        );
        let props = parse_getprop("[ro.build.flyme.version]: [10]\n[ro.empty]: []\ngarbage\n");
        assert_eq!(props.get("ro.build.flyme.version").map(String::as_str), Some("10"));
        assert_eq!(props.get("ro.empty").map(String::as_str), Some(""));
    }

    /// ROM 识别表每一行与兜底（T-09）。
    #[test]
    fn rom_table() {
        for rom in ROMS {
            for prop in rom.props {
                let props = BTreeMap::from([((*prop).to_owned(), "1".to_owned())]);
                assert_eq!(rom_of(&props).map(|r| r.name), Some(rom.name), "{prop}");
            }
        }
        assert!(rom_of(&BTreeMap::from([("ro.build.flyme.version".to_owned(), String::new())])).is_none(), "空值不算");
        assert!(rom_of(&BTreeMap::new()).is_none());
    }

    #[test]
    fn blocked_packages_from_log() {
        let b = blocked_packages(&format!("{FLYME_BLOCK_LOG}{}", FLYME_BLOCK_LOG.replace("17:45:35.977", "18:01:02.003")));
        let got: Vec<(&str, &str)> = b.iter().map(|b| (b.package.as_str(), b.last_seen.as_str())).collect();
        assert_eq!(got, [("dev.appmcp.sample.android", "10-02 18:01:02.003"), ("com.unrelated.app", "10-02 17:45:36.001")]);
        assert!(blocked_packages("W ActivityManager: Unable to start service Intent { act=dev.appmcp.TOOLS } U=0: not found").is_empty());
        let big = format!("{}{}", "日".repeat(MAX_LOG_BYTES), FLYME_BLOCK_LOG);
        assert_eq!(blocked_packages(log_tail(&big)).len(), 2, "截取末尾不切断多字节字符");
    }

    #[test]
    fn healthy_device_is_ok_with_rom_note() {
        let c = evaluate(&Devices::Probed { ready: vec![device(vec![sample()], "", flyme_props())], not_ready: vec![] });
        assert_eq!(c.status, Level::Ok, "{}", c.summary);
        assert!(c.summary.contains("MEIZU 18 Pro，Flyme") && c.summary.contains("dev.appmcp.TOOLS：dev.appmcp.sample.android/dev.appmcp.android.ToolsService「app-mcp 示例」"), "{}", c.summary);
        assert!(c.summary.contains("独立 Hub App：dev.appmcp.hub.app/"), "{}", c.summary);
        assert!(c.summary.contains("默认拦截第三方 App 之间的服务绑定"), "{}", c.summary);
        assert!(c.hint.as_deref().is_some_and(|h| h.contains("耗电和后台")));
        assert_eq!(c.code, None);
    }

    #[test]
    fn blocked_bind_warns_with_code_and_settings_hint() {
        let c = evaluate(&Devices::Probed { ready: vec![device(vec![sample()], FLYME_BLOCK_LOG, flyme_props())], not_ready: vec![] });
        assert_eq!(c.status, Level::Warn, "{}", c.summary);
        assert_eq!(c.code, Some(codes::ACTIVATION_BLOCKED));
        assert!(c.summary.contains("dev.appmcp.sample.android（最近 10-02 17:45:35.977）") && !c.summary.contains("com.unrelated.app"), "只报相关的包：{}", c.summary);
        assert!(c.hint.as_deref().is_some_and(|h| h.contains("耗电和后台") && h.contains("doctor 不修改设置")), "{:?}", c.hint);
        // 未知 ROM：通用指引。
        let c = evaluate(&Devices::Probed { ready: vec![device(vec![sample()], FLYME_BLOCK_LOG, BTreeMap::new())], not_ready: vec![] });
        assert!(c.hint.as_deref().is_some_and(|h| h.contains("自启动 / 关联启动开关")), "{:?}", c.hint);
    }

    /// Service 声明规则表每一行（T-09）。
    #[test]
    fn declaration_rules() {
        type Mutate = fn(&mut ServiceDecl);
        let cases: &[(Mutate, Level, &str)] = &[
            (|d| d.exported = Some(false), Level::Error, "未导出"),
            (|d| d.enabled = Some(false), Level::Warn, "已被禁用"),
            (|d| d.permission = Some("x.PERM".into()), Level::Warn, "要求权限 x.PERM"),
        ];
        for (mutate, level, needle) in cases {
            let mut d = sample();
            mutate(&mut d);
            let c = evaluate(&Devices::Probed { ready: vec![device(vec![d], "", BTreeMap::new())], not_ready: vec![] });
            assert_eq!(c.status, *level, "{needle}: {}", c.summary);
            assert!(c.summary.contains(needle) && c.hint.is_some(), "{needle}: {}", c.summary);
        }
        // Hub App 只核对导出。
        let mut hub = parse_query_services(HUB_OUTPUT).unwrap();
        hub[0].enabled = Some(false);
        let mut d = device(vec![], "", BTreeMap::new());
        d.hub = Ok(hub);
        assert_eq!(evaluate(&Devices::Probed { ready: vec![d], not_ready: vec![] }).status, Level::Ok);
    }

    #[test]
    fn skip_and_failure_paths() {
        assert_eq!(evaluate(&Devices::NoAdb).status, Level::Skip);
        assert_eq!(evaluate(&Devices::Failed("x".into())).status, Level::Skip);
        let none = evaluate(&Devices::Probed { ready: vec![], not_ready: vec![] });
        assert_eq!((none.status, none.summary.as_str()), (Level::Skip, "没有已连接的 Android 设备"));
        let unauth = evaluate(&Devices::Probed { ready: vec![], not_ready: vec![("abc".into(), "unauthorized".into())] });
        assert!(unauth.summary.contains("abc（unauthorized）") && unauth.hint.is_some());
        let failed = DeviceProbe {
            serial: "s".into(),
            tools: Err("Unknown command".into()),
            hub: Ok(vec![]),
            props: BTreeMap::new(),
            log: Err("超时".into()),
        };
        let c = evaluate(&Devices::Probed { ready: vec![failed], not_ready: vec![] });
        assert_eq!(c.status, Level::Info, "{}", c.summary);
        assert!(c.summary.contains("查询 dev.appmcp.TOOLS 失败：Unknown command"), "{}", c.summary);
    }

    /// 假 adb（脚本）：命令按参数分派，验证探测流程与超时（不需要真实设备）。
    #[cfg(unix)]
    #[tokio::test]
    async fn probes_with_fake_adb() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("app-mcp-doctor-adb-{:032x}", rand::random::<u128>()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("tools.txt"), TOOLS_OUTPUT).unwrap();
        std::fs::write(dir.join("log.txt"), FLYME_BLOCK_LOG).unwrap();
        let script = format!(
            "#!/bin/sh\n\
             case \"$*\" in\n\
             devices) printf 'List of devices attached\\nSER1\\tdevice\\nSER2\\toffline\\n' ;;\n\
             *dev.appmcp.TOOLS*) cat '{d}/tools.txt' ;;\n\
             *dev.appmcp.HUB*) echo 'No services found' ;;\n\
             *getprop*) echo '[ro.build.flyme.version]: [10]' ;;\n\
             *logcat*) cat '{d}/log.txt' ;;\n\
             esac\n",
            d = dir.display()
        );
        let adb = dir.join("adb");
        std::fs::write(&adb, script).unwrap();
        std::fs::set_permissions(&adb, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut env = NamingEnv {
            registration_dirs: vec![],
            dbus_service_dirs: vec![],
            dbus_address: None,
            adb: Some(adb.clone()),
            timeout: std::time::Duration::from_secs(5),
        };
        let c = check(&env).await;
        assert_eq!((c.status, c.code), (Level::Warn, Some(codes::ACTIVATION_BLOCKED)), "{}", c.summary);
        assert!(c.summary.contains("SER1（未知型号，Flyme）") && c.summary.contains("未就绪：SER2（offline）"), "{}", c.summary);
        assert!(c.summary.contains("没有独立 Hub App"), "{}", c.summary);

        // adb 卡住：在超时内返回。
        std::fs::write(&adb, "#!/bin/sh\nsleep 10\n").unwrap();
        env.timeout = std::time::Duration::from_millis(300);
        let started = std::time::Instant::now();
        let c = check(&env).await;
        assert_eq!(c.status, Level::Skip, "{}", c.summary);
        assert!(c.summary.contains("没有返回"), "{}", c.summary);
        assert!(started.elapsed() < std::time::Duration::from_secs(3));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
