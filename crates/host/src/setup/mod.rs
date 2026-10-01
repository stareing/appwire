//! `app-mcp-host setup` / `uninstall`：一条命令完成「二进制就位 → 登录自启 → 等 `/healthz` → 写入已装 Agent 的 MCP 配置
//! → doctor 自检」，以及按 `<home>/setup.json` 撤销（`docs/plans/13-out-of-box.md` D3）。
//!
//! - [`binary`]：从 npx / uvx 等包管理器目录运行时复制到 `<home>/bin/`；
//! - [`agents`]：Agent 策略表（每种 Agent 一个模块）；
//! - [`record`]：改动清单 `setup.json`；[`files`]：备份 / 指纹 / 原子写入；
//! - [`ops`]：外部操作边界（Agent 命令、服务安装、`/healthz`、doctor），测试以假实现替换。
//!
//! @invariant 幂等：已是目标状态的步骤不重复写入；已有不同的同名 Agent 条目且不是本程序写入的 → 不覆盖（需 `--force`）。
//! @invariant 每次写 Agent 配置前备份被修改的文件，写入后回读校验，失败即回滚（文件未再变化时恢复备份，否则只删本条目）。
//! @side-effect 非 dry-run 时写 `<home>/bin/`、`<home>/backups/`、`<home>/setup.json`、登录自启项与各 Agent 配置。

pub mod agents;
pub mod binary;
pub mod files;
pub mod ops;
pub mod record;

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::config::{AppHome, AuthMode};
use crate::doctor::{Level, Report};
use agents::{AgentEnv, AgentId, AgentSelection, AgentSpec, Existing, MCP_SERVER_NAME, Method};
use binary::BinaryPlan;
use ops::{CommandRunner, HostOps};
use record::{AgentRecord, BinaryRecord, ServiceRecord, SetupRecord};

/// 备份目录（`<home>/backups`）。
pub const BACKUP_DIR: &str = "backups";

#[derive(Clone, Debug)]
pub struct SetupOptions {
    pub home: AppHome,
    pub agents: AgentSelection,
    /// 替换已有的不同同名条目。
    pub force: bool,
    /// 只输出计划，不做任何修改。
    pub dry_run: bool,
}

#[derive(Clone, Debug)]
pub struct UninstallOptions {
    pub home: AppHome,
    /// 同时删除 `<home>/bin`。
    pub purge: bool,
    pub dry_run: bool,
}

/// 一个步骤的结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum StepStatus {
    Done,
    Unchanged,
    Planned,
    Skipped,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    pub status: StepStatus,
    pub detail: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<String>,
}

impl Step {
    fn new(status: StepStatus, detail: impl Into<String>) -> Self {
        Self { status, detail: detail.into(), messages: Vec::new() }
    }
}

/// 某个 Agent 的处理结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentOutcome {
    /// 新增条目。
    Added,
    /// 更新本程序以前写入的条目（Host 地址变化）。
    Updated,
    /// `--force` 替换了不同的同名条目。
    Replaced,
    Unchanged,
    Planned,
    /// 已有不同的同名条目，未修改。
    Conflict,
    NotDetected,
    /// 只能手动配置（见 `manual`）。
    Manual,
    /// 未处理（Host 未就绪、令牌策略等，见 `detail`）。
    Skipped,
    Failed,
    /// uninstall：整文件恢复为 setup 之前的备份。
    Restored,
    /// uninstall：只删除了本条目。
    Removed,
    /// uninstall：条目已被用户改动，保留不动。
    Kept,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentReport {
    pub agent: AgentId,
    pub display: &'static str,
    pub outcome: AgentOutcome,
    pub detail: String,
    /// 手动配置说明。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manual: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backup: Option<PathBuf>,
    /// 写入方式的依据。
    pub evidence: &'static str,
}

impl AgentReport {
    fn new(spec: &AgentSpec, outcome: AgentOutcome, detail: impl Into<String>) -> Self {
        Self { agent: spec.id, display: spec.display, outcome, detail: detail.into(), manual: None, backup: None, evidence: spec.evidence }
    }
    fn manual(mut self, m: String) -> Self {
        self.manual = Some(m);
        self
    }
    fn failed(&self) -> bool {
        matches!(self.outcome, AgentOutcome::Failed | AgentOutcome::Conflict)
    }
}

/// doctor 结果摘要。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DoctorSummary {
    pub errors: usize,
    pub warnings: usize,
    /// 错误项（`标题：结论`）。
    pub problems: Vec<String>,
}

impl DoctorSummary {
    pub fn from_report(r: &Report) -> Self {
        let count = |l: Level| r.checks.iter().filter(|c| c.status == l).count();
        Self {
            errors: count(Level::Error),
            warnings: count(Level::Warn),
            problems: r
                .checks
                .iter()
                .filter(|c| c.status == Level::Error)
                .map(|c| format!("{}：{}", c.title, c.summary))
                .collect(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupReport {
    pub dry_run: bool,
    pub home: PathBuf,
    pub binary: Step,
    pub service: Step,
    /// Host 的 MCP 地址（取自登记文件中的实际监听地址）。
    pub mcp_url: Option<String>,
    pub agents: Vec<AgentReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doctor: Option<DoctorSummary>,
    /// 未自动配置的客户端的通用说明。
    pub other_clients: String,
}

impl SetupReport {
    /// 全部成功（无失败步骤、无冲突 / 失败的 Agent、doctor 无错误）。
    pub fn ok(&self) -> bool {
        self.binary.status != StepStatus::Failed
            && self.service.status != StepStatus::Failed
            && !self.agents.iter().any(AgentReport::failed)
            && self.doctor.as_ref().is_none_or(|d| d.errors == 0)
    }

    pub fn render(&self) -> String {
        let mut out = format!(
            "AppWire setup（配置目录 {}）{}\n",
            self.home.display(),
            if self.dry_run { "：--dry-run，只列出计划，未做任何修改" } else { "" },
        );
        render_step(&mut out, "二进制", &self.binary);
        render_step(&mut out, "登录自启", &self.service);
        out.push_str(&format!("MCP 地址：{}\n", self.mcp_url.as_deref().unwrap_or("（未取得）")));
        render_agents(&mut out, &self.agents);
        if let Some(d) = &self.doctor {
            out.push_str(&format!("doctor：错误 {}，注意 {}", d.errors, d.warnings));
            out.push_str(if d.errors + d.warnings > 0 { "（app-mcp-host doctor 查看详情）\n" } else { "\n" });
            for p in &d.problems {
                out.push_str(&format!("  - {p}\n"));
            }
        }
        out.push_str(&format!("{}\n", self.other_clients));
        out
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UninstallReport {
    pub dry_run: bool,
    pub home: PathBuf,
    /// 是否找到 `setup.json`。
    pub record_found: bool,
    pub agents: Vec<AgentReport>,
    pub service: Step,
    pub binary: Step,
}

impl UninstallReport {
    pub fn ok(&self) -> bool {
        self.service.status != StepStatus::Failed
            && self.binary.status != StepStatus::Failed
            && !self.agents.iter().any(AgentReport::failed)
    }

    pub fn render(&self) -> String {
        let mut out = format!(
            "AppWire uninstall（配置目录 {}）{}\n",
            self.home.display(),
            if self.dry_run { "：--dry-run，只列出计划，未做任何修改" } else { "" },
        );
        if !self.record_found {
            out.push_str(&format!(
                "没有 {}：setup 未在此配置目录做过改动，不撤销任何内容（手动安装的服务用 `app-mcp-host service uninstall` 卸载）\n",
                record::SETUP_FILE
            ));
        }
        render_agents(&mut out, &self.agents);
        render_step(&mut out, "登录自启", &self.service);
        render_step(&mut out, "二进制", &self.binary);
        out
    }
}

fn status_label(s: StepStatus) -> &'static str {
    match s {
        StepStatus::Done => "完成",
        StepStatus::Unchanged => "无变化",
        StepStatus::Planned => "计划",
        StepStatus::Skipped => "跳过",
        StepStatus::Failed => "失败",
    }
}

fn outcome_label(o: AgentOutcome) -> &'static str {
    match o {
        AgentOutcome::Added => "已添加",
        AgentOutcome::Updated => "已更新",
        AgentOutcome::Replaced => "已替换",
        AgentOutcome::Unchanged => "已是最新",
        AgentOutcome::Planned => "计划",
        AgentOutcome::Conflict => "冲突",
        AgentOutcome::NotDetected => "未检测到",
        AgentOutcome::Manual => "需手动",
        AgentOutcome::Skipped => "跳过",
        AgentOutcome::Failed => "失败",
        AgentOutcome::Restored => "已恢复备份",
        AgentOutcome::Removed => "已删除条目",
        AgentOutcome::Kept => "保留",
    }
}

fn render_step(out: &mut String, title: &str, s: &Step) {
    for m in &s.messages {
        out.push_str(&format!("  {m}\n"));
    }
    out.push_str(&format!("{title}：[{}] {}\n", status_label(s.status), s.detail));
}

fn render_agents(out: &mut String, agents: &[AgentReport]) {
    if agents.is_empty() {
        out.push_str("Agent：未处理任何 Agent\n");
        return;
    }
    out.push_str("Agent：\n");
    for a in agents {
        out.push_str(&format!("  {}：[{}] {}\n", a.display, outcome_label(a.outcome), a.detail));
        if let Some(m) = &a.manual {
            out.push_str(&format!("      手动配置：{m}\n"));
        }
        if let Some(b) = &a.backup {
            out.push_str(&format!("      备份：{}\n", b.display()));
        }
    }
}

/// 写入决定（纯函数）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    Unchanged,
    Add,
    /// 更新本程序以前写入的条目。
    Update,
    /// `--force` 替换（附原条目说明）。
    Replace(String),
    /// 不修改（附原因）。
    Conflict(String),
}

/// @input recorded_url 本程序上次写入该 Agent 的 URL（`setup.json`）。
pub fn decide(existing: Option<&Existing>, url: &str, recorded_url: Option<&str>, force: bool) -> Decision {
    let Some(e) = existing else {
        return Decision::Add;
    };
    if e.url.as_deref() == Some(url) {
        return Decision::Unchanged;
    }
    if !e.replaceable {
        return Decision::Conflict(format!("已有同名条目「{}」，不在可写入的作用域，未修改", e.summary));
    }
    if e.url.is_some() && e.url.as_deref() == recorded_url {
        return Decision::Update;
    }
    if force {
        return Decision::Replace(e.summary.clone());
    }
    Decision::Conflict(format!("已有不同的同名条目「{}」；确认替换请加 --force", e.summary))
}

/// 撤销一条写入的方式。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Undo {
    /// 文件自写入后未变化：整文件恢复为备份（原本不存在则删除）。
    Restored,
    /// 文件已有其他变化：只删除了本条目。
    Removed,
    /// 条目已不存在。
    AlreadyGone,
    /// 条目已被改为别的内容（附说明），不动。
    LeftModified(String),
}

/// 撤销：文件指纹仍为 `written_hash` 时恢复备份，否则只删除 URL 仍为 `url` 的同名条目。
pub async fn undo(
    spec: &AgentSpec,
    env: &AgentEnv,
    runner: &impl CommandRunner,
    file: Option<&Path>,
    backup: Option<&Path>,
    written_hash: Option<&str>,
    url: &str,
) -> anyhow::Result<Undo> {
    if let (Some(f), Some(h)) = (file, written_hash)
        && files::fingerprint(f)?.as_deref() == Some(h)
    {
        files::restore(f, backup)?;
        return Ok(Undo::Restored);
    }
    match agents::read_existing(spec, env, runner, MCP_SERVER_NAME).await? {
        None => Ok(Undo::AlreadyGone),
        Some(e) if e.url.as_deref() == Some(url) => {
            agents::remove(spec, env, runner, MCP_SERVER_NAME).await?;
            Ok(Undo::Removed)
        }
        Some(e) => Ok(Undo::LeftModified(e.summary)),
    }
}

fn mcp_url(listen: &str) -> String {
    format!("http://{}/mcp", crate::probe_addr(listen))
}

fn other_clients(url: Option<&str>) -> String {
    format!(
        "其他 MCP 客户端（Windsurf、Claude Desktop 等）：手动添加 HTTP（Streamable HTTP）MCP 服务器 {} = {}；`--agents windsurf,claude-desktop --dry-run` 查看说明",
        MCP_SERVER_NAME,
        url.unwrap_or("<MCP 地址>")
    )
}

/// 共享的上下文。
struct Ctx<'a, R> {
    home: &'a AppHome,
    env: &'a AgentEnv,
    runner: &'a R,
    force: bool,
    dry_run: bool,
}

/// 执行 setup。只有读取 `setup.json` 失败时返回错误；各步骤的失败写进报告（[`SetupReport::ok`]）。
///
/// @input exe 当前运行的主程序（`app-mcp-host`）。
pub async fn setup<H: HostOps, R: CommandRunner>(
    opts: &SetupOptions,
    host: &H,
    runner: &R,
    env: &AgentEnv,
    exe: &Path,
) -> anyhow::Result<SetupReport> {
    let home = &opts.home;
    let mut record = SetupRecord::load(&home.dir)?.unwrap_or_default();
    let settings = host.settings(home)?;

    // 1. 二进制
    let plan = BinaryPlan::new(exe, &home.dir);
    let (binary, binary_changed) = binary_step(&plan, opts.dry_run, &mut record, home)?;

    // 2. 登录自启 + 等 /healthz
    let service_exe = crate::service::background_exe_for(&plan.stable_exe());
    let mut registry = None;
    let service = if binary.status == StepStatus::Failed {
        Step::new(StepStatus::Skipped, "二进制未就位，未安装服务")
    } else if opts.dry_run {
        Step::new(
            StepStatus::Planned,
            format!(
                "将安装登录自启服务（{}）：{} serve --home {}，并等待 /healthz 就绪",
                host.service_location(),
                service_exe.display(),
                home.dir.display()
            ),
        )
    } else {
        match host.install_service(home, &service_exe, binary_changed).await {
            Ok(out) => {
                record.service = Some(ServiceRecord { location: out.location.clone(), exe: service_exe.clone() });
                record.save(&home.dir)?;
                let mut step = match out.registry {
                    Ok(reg) => {
                        let detail = format!("已安装（{}），Host 运行中（pid {}）", out.location, reg.identity.pid);
                        registry = Some(reg);
                        Step::new(StepStatus::Done, detail)
                    }
                    Err(why) => Step::new(StepStatus::Failed, format!("已安装（{}），但 10 秒内未就绪：{why}", out.location)),
                };
                step.messages = out.messages;
                step
            }
            Err(e) => Step::new(StepStatus::Failed, format!("{e:#}")),
        }
    };
    // 服务没装上时，可能有手动运行的 serve：以实际运行的实例为准。
    if registry.is_none() {
        registry = host.running(home).await;
    }

    // 3. MCP 地址：登记文件中的实际监听地址
    let url = registry.as_ref().and_then(|r| r.listen.as_deref()).map(mcp_url);
    let (agent_url, url_note) = match (&url, opts.dry_run) {
        (Some(u), _) => (Some(u.clone()), None),
        (None, true) => (Some(mcp_url(&settings.listen)), Some("Host 未运行，按配置推测；实际地址以启动后的 endpoints.json 为准")),
        (None, false) => (None, None),
    };

    // 4. Agent
    let ctx = Ctx { home, env, runner, force: opts.force, dry_run: opts.dry_run };
    let mut reports = Vec::new();
    for (spec, explicit) in opts.agents.specs() {
        let rep = match (&agent_url, settings.auth) {
            (None, _) => AgentReport::new(spec, AgentOutcome::Skipped, "Host 未就绪，未取得实际地址；Host 运行后重新执行 setup"),
            (Some(u), AuthMode::All) => AgentReport::new(
                spec,
                AgentOutcome::Skipped,
                "令牌策略为 all：客户端需携带 Authorization: Bearer <令牌>，setup 不把令牌写入 Agent 配置",
            )
            .manual(format!("{}（另加请求头 Authorization: Bearer $(app-mcp-host token)）", (spec.manual)(MCP_SERVER_NAME, u))),
            (Some(u), _) => configure_agent(spec, explicit, &ctx, u, &mut record).await,
        };
        reports.push(rep);
    }

    // 5. doctor
    let doctor = if opts.dry_run {
        None
    } else {
        Some(match host.doctor(home).await {
            Ok(r) => DoctorSummary::from_report(&r),
            Err(e) => DoctorSummary { errors: 1, warnings: 0, problems: vec![format!("doctor 运行失败：{e:#}")] },
        })
    };

    let mcp_url_shown = match (url_note, &agent_url) {
        (Some(note), Some(u)) => Some(format!("{u}（{note}）")),
        _ => url.clone(),
    };
    Ok(SetupReport {
        dry_run: opts.dry_run,
        home: home.dir.clone(),
        binary,
        service,
        mcp_url: mcp_url_shown,
        agents: reports,
        doctor,
        other_clients: other_clients(agent_url.as_deref()),
    })
}

/// 返回 (步骤结果, 二进制内容是否有变化)。
fn binary_step(plan: &BinaryPlan, dry_run: bool, record: &mut SetupRecord, home: &AppHome) -> anyhow::Result<(Step, bool)> {
    let Some(dir) = &plan.target_dir else {
        return Ok((Step::new(StepStatus::Unchanged, format!("原地使用 {}（不在包管理器目录中）", plan.current.display())), false));
    };
    if dry_run {
        return Ok((
            Step::new(StepStatus::Planned, format!("{} 位于包管理器目录，将复制到 {}", plan.current.display(), dir.display())),
            false,
        ));
    }
    match plan.install() {
        Ok((targets, changed)) => {
            record.binary = Some(BinaryRecord { dir: dir.clone(), files: targets });
            record.save(&home.dir)?;
            let status = if changed { StepStatus::Done } else { StepStatus::Unchanged };
            Ok((Step::new(status, format!("{}（来源 {}）", plan.stable_exe().display(), plan.current.display())), changed))
        }
        Err(e) => Ok((Step::new(StepStatus::Failed, format!("{e:#}")), false)),
    }
}

async fn configure_agent<R: CommandRunner>(
    spec: &'static AgentSpec,
    explicit: bool,
    ctx: &Ctx<'_, R>,
    url: &str,
    record: &mut SetupRecord,
) -> AgentReport {
    let manual = (spec.manual)(MCP_SERVER_NAME, url);
    if matches!(spec.method, Method::Manual) {
        return AgentReport::new(spec, AgentOutcome::Manual, "写入位置 / 格式无法从官方文档确认，不自动修改").manual(manual);
    }
    if agents::detect(spec, ctx.env, ctx.runner).is_none() {
        let detail = if explicit { "已指定但未检测到（未安装？）" } else { "未安装" };
        return AgentReport::new(spec, AgentOutcome::NotDetected, detail);
    }
    let existing = match agents::read_existing(spec, ctx.env, ctx.runner, MCP_SERVER_NAME).await {
        Ok(e) => e,
        Err(e) => return AgentReport::new(spec, AgentOutcome::Failed, format!("无法读取现有配置：{e:#}")).manual(manual),
    };
    let recorded = record.agent(spec.id).map(|r| r.url.clone());
    let decision = decide(existing.as_ref(), url, recorded.as_deref(), ctx.force);
    let (outcome, action) = match &decision {
        Decision::Unchanged => return AgentReport::new(spec, AgentOutcome::Unchanged, format!("{MCP_SERVER_NAME} = {url}")),
        Decision::Conflict(why) => return AgentReport::new(spec, AgentOutcome::Conflict, why.clone()).manual(manual),
        Decision::Add => (AgentOutcome::Added, format!("添加 {MCP_SERVER_NAME} = {url}")),
        Decision::Update => (AgentOutcome::Updated, format!("更新本程序写入的 {MCP_SERVER_NAME} 为 {url}")),
        Decision::Replace(prev) => (AgentOutcome::Replaced, format!("替换「{prev}」为 {url}")),
    };
    if ctx.dry_run {
        return AgentReport::new(spec, AgentOutcome::Planned, format!("将{action}"));
    }
    match write_verified(spec, ctx, url, &decision, record).await {
        Ok(backup) => {
            let mut r = AgentReport::new(spec, outcome, action);
            r.backup = backup;
            r
        }
        Err(e) => AgentReport::new(spec, AgentOutcome::Failed, format!("{e:#}")).manual(manual),
    }
}

/// 备份 → 写入 → 回读校验 → 记录；失败时回滚。返回记录中的备份位置。
async fn write_verified<R: CommandRunner>(
    spec: &'static AgentSpec,
    ctx: &Ctx<'_, R>,
    url: &str,
    decision: &Decision,
    record: &mut SetupRecord,
) -> anyhow::Result<Option<PathBuf>> {
    let file = (spec.config_file)(ctx.env);
    let file = file.as_deref();
    let before = file.map(files::fingerprint).transpose()?.flatten();
    let backup_dir = ctx.home.dir.join(BACKUP_DIR);
    let pre = match file {
        Some(f) => files::backup(f, &backup_dir, spec.key)?,
        None => None,
    };
    let replacing = matches!(decision, Decision::Update | Decision::Replace(_));
    let written = agents::write(spec, ctx.env, ctx.runner, MCP_SERVER_NAME, url, replacing).await;
    let after = file.map(files::fingerprint).transpose()?.flatten();
    let verified = match written {
        Err(e) => Err(e),
        Ok(()) => match agents::read_existing(spec, ctx.env, ctx.runner, MCP_SERVER_NAME).await {
            Ok(Some(e)) if e.url.as_deref() == Some(url) => Ok(()),
            Ok(other) => Err(anyhow::anyhow!(
                "写入后回读校验不符：{}",
                other.map(|e| e.summary).unwrap_or_else(|| "条目不存在".into())
            )),
            Err(e) => Err(e.context("写入后回读失败")),
        },
    };
    if let Err(e) = verified {
        let rollback = if after == before {
            "配置未变化".to_owned()
        } else {
            match undo(spec, ctx.env, ctx.runner, file, pre.as_deref(), after.as_deref(), url).await {
                Ok(Undo::Restored) => "已恢复备份".to_owned(),
                Ok(Undo::Removed) => "已删除写入的条目".to_owned(),
                Ok(Undo::AlreadyGone) => "条目不存在，无需回滚".to_owned(),
                Ok(Undo::LeftModified(s)) => format!("条目已是「{s}」，未回滚"),
                Err(re) => format!("回滚失败：{re:#}"),
            }
        };
        let keep = pre.as_ref().map(|p| format!("；写入前备份 {}", p.display())).unwrap_or_default();
        return Err(e.context(format!("{rollback}{keep}")));
    }
    let written_hash = file.map(files::fingerprint).transpose()?.flatten();
    record.upsert_agent(AgentRecord {
        agent: spec.id,
        name: MCP_SERVER_NAME.to_owned(),
        url: url.to_owned(),
        file: file.map(Path::to_path_buf),
        backup: pre.clone(),
        written_hash,
        replaced: match decision {
            Decision::Replace(prev) => Some(prev.clone()),
            _ => None,
        },
    });
    record.save(&ctx.home.dir)?;
    let kept = record.agent(spec.id).and_then(|r| r.backup.clone());
    // 记录保留的是更早的备份（代表 setup 之前的原状）：本次的备份不再需要。
    if let Some(p) = &pre
        && kept.as_ref() != Some(p)
    {
        let _ = std::fs::remove_file(p);
    }
    Ok(kept)
}

/// 执行 uninstall：只撤销 `setup.json` 中记录的改动。
pub async fn uninstall<H: HostOps, R: CommandRunner>(
    opts: &UninstallOptions,
    host: &H,
    runner: &R,
    env: &AgentEnv,
) -> anyhow::Result<UninstallReport> {
    let home = &opts.home;
    let loaded = SetupRecord::load(&home.dir)?;
    let record_found = loaded.is_some();
    let mut record = loaded.unwrap_or_default();

    let mut reports = Vec::new();
    let mut remaining = Vec::new();
    for rec in record.agents.clone().into_iter().rev() {
        let (rep, done) = undo_agent(&rec, opts.dry_run, env, runner).await;
        reports.push(rep);
        if !done {
            remaining.push(rec);
        }
    }
    remaining.reverse();
    if !opts.dry_run {
        record.agents = remaining;
        record.save(&home.dir)?;
        // 备份目录为空时删除（不为空说明有保留的备份）。
        let _ = std::fs::remove_dir(home.dir.join(BACKUP_DIR));
    }

    let service = match (&record.service, opts.dry_run) {
        (None, _) => Step::new(StepStatus::Skipped, "setup 未安装服务（或已撤销）"),
        (Some(s), true) => Step::new(StepStatus::Planned, format!("将卸载登录自启服务（{}）", s.location)),
        (Some(s), false) => match host.uninstall_service(home).await {
            Ok(removed) => {
                let detail = if removed { format!("已卸载（{}）", s.location) } else { "服务已不存在".to_owned() };
                record.service = None;
                record.save(&home.dir)?;
                Step::new(StepStatus::Done, detail)
            }
            Err(e) => Step::new(StepStatus::Failed, format!("{e:#}")),
        },
    };

    let bin_dir = home.dir.join(binary::BIN_DIR);
    let binary = match (opts.purge, opts.dry_run, &record.binary) {
        (false, _, Some(b)) => Step::new(StepStatus::Skipped, format!("保留 {}（--purge 删除）", b.dir.display())),
        (false, _, None) => Step::new(StepStatus::Skipped, "setup 未复制二进制"),
        (true, _, _) if !bin_dir.exists() => {
            record.binary = None;
            if !opts.dry_run {
                record.save(&home.dir)?;
            }
            Step::new(StepStatus::Unchanged, format!("{} 不存在", bin_dir.display()))
        }
        (true, true, _) => Step::new(StepStatus::Planned, format!("将删除 {}", bin_dir.display())),
        (true, false, _) if service.status == StepStatus::Failed => {
            Step::new(StepStatus::Skipped, "服务卸载失败，保留二进制（自启项仍指向它）")
        }
        (true, false, _) => match std::fs::remove_dir_all(&bin_dir) {
            Ok(()) => {
                record.binary = None;
                record.save(&home.dir)?;
                Step::new(StepStatus::Done, format!("已删除 {}", bin_dir.display()))
            }
            Err(e) => Step::new(StepStatus::Failed, format!("删除 {} 失败：{e}", bin_dir.display())),
        },
    };

    Ok(UninstallReport { dry_run: opts.dry_run, home: home.dir.clone(), record_found, agents: reports, service, binary })
}

/// 撤销一条 Agent 记录；返回 (报告, 是否可从清单中删除)。
async fn undo_agent<R: CommandRunner>(rec: &AgentRecord, dry_run: bool, env: &AgentEnv, runner: &R) -> (AgentReport, bool) {
    let spec = agents::spec(rec.agent);
    let unchanged_since_write = match (&rec.file, &rec.written_hash) {
        (Some(f), Some(h)) => files::fingerprint(f).ok().flatten().as_deref() == Some(h.as_str()),
        _ => false,
    };
    if dry_run {
        let plan = if unchanged_since_write {
            format!("将恢复 {} 为 setup 之前的状态", rec.file.as_deref().map(Path::display).map(|d| d.to_string()).unwrap_or_default())
        } else {
            format!("将删除条目 {}（URL 仍为 {} 时）", rec.name, rec.url)
        };
        return (AgentReport::new(spec, AgentOutcome::Planned, plan), false);
    }
    let result = undo(spec, env, runner, rec.file.as_deref(), rec.backup.as_deref(), rec.written_hash.as_deref(), &rec.url).await;
    let mut rep = match result {
        Ok(Undo::Restored) => {
            let what = match &rec.backup {
                Some(_) => "文件自 setup 写入后未变化，已恢复为写入前的备份",
                None => "文件为 setup 创建且之后未变化，已删除",
            };
            AgentReport::new(spec, AgentOutcome::Restored, what)
        }
        Ok(Undo::Removed) => AgentReport::new(spec, AgentOutcome::Removed, format!("文件已有其他变化，只删除了 {}", rec.name)),
        Ok(Undo::AlreadyGone) => AgentReport::new(spec, AgentOutcome::Removed, format!("{} 已不存在", rec.name)),
        Ok(Undo::LeftModified(s)) => AgentReport::new(spec, AgentOutcome::Kept, format!("{} 已被改为「{s}」，未修改", rec.name)),
        Err(e) => return (AgentReport::new(spec, AgentOutcome::Failed, format!("{e:#}")), false),
    };
    // 被 --force 替换掉的原条目：只删本条目时原条目无法自动还原，保留备份并提示。
    match (&rec.replaced, &rec.backup, &rep.outcome) {
        (Some(prev), Some(b), AgentOutcome::Removed | AgentOutcome::Kept) => {
            rep.detail.push_str(&format!("；setup --force 曾替换原条目「{prev}」，可从备份手动恢复"));
            rep.backup = Some(b.clone());
        }
        (_, Some(b), _) => {
            let _ = std::fs::remove_file(b);
        }
        _ => {}
    }
    (rep, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    const URL: &str = "http://127.0.0.1:7717/mcp";

    #[test]
    fn decision_table() {
        let ours = Existing::http(URL);
        let old = Existing::http("http://127.0.0.1:7737/mcp");
        let stdio = Existing { url: None, summary: "stdio npx".into(), replaceable: true };
        let foreign = Existing { url: Some("http://x/mcp".into()), summary: "x".into(), replaceable: false };
        assert_eq!(decide(None, URL, None, false), Decision::Add);
        assert_eq!(decide(Some(&ours), URL, None, false), Decision::Unchanged);
        assert_eq!(decide(Some(&ours), URL, None, true), Decision::Unchanged);
        // 本程序写入的旧地址：不需要 --force
        assert_eq!(decide(Some(&old), URL, Some("http://127.0.0.1:7737/mcp"), false), Decision::Update);
        // 他人写入：需要 --force
        assert!(matches!(decide(Some(&old), URL, None, false), Decision::Conflict(_)));
        assert_eq!(decide(Some(&old), URL, None, true), Decision::Replace(old.summary.clone()));
        assert!(matches!(decide(Some(&stdio), URL, Some(URL), false), Decision::Conflict(_)));
        assert!(matches!(decide(Some(&stdio), URL, None, true), Decision::Replace(_)));
        // 不可写作用域：--force 也不改
        assert!(matches!(decide(Some(&foreign), URL, None, true), Decision::Conflict(_)));
    }

    #[test]
    fn report_ok_rules() {
        let spec = agents::spec(AgentId::Cursor);
        let mut r = SetupReport {
            dry_run: false,
            home: "/h".into(),
            binary: Step::new(StepStatus::Unchanged, ""),
            service: Step::new(StepStatus::Done, ""),
            mcp_url: Some(URL.into()),
            agents: vec![AgentReport::new(spec, AgentOutcome::NotDetected, "")],
            doctor: Some(DoctorSummary { errors: 0, warnings: 2, problems: vec![] }),
            other_clients: String::new(),
        };
        assert!(r.ok());
        r.agents.push(AgentReport::new(spec, AgentOutcome::Conflict, ""));
        assert!(!r.ok());
        r.agents.pop();
        r.doctor = Some(DoctorSummary { errors: 1, warnings: 0, problems: vec!["x".into()] });
        assert!(!r.ok());
        assert!(r.render().contains("doctor：错误 1"));
    }
}
