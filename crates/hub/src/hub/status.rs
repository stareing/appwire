//! 运行状态（`/status`、`Hub::status`）：诊断记录、App 与实例状态、Agent 任务快照。

use std::collections::HashMap;
use std::time::Duration;

use crate::call;
use crate::limits::LimitOverrides;
use crate::task::CallerKey;
use crate::types::{
    AgentTaskStatus, AppKind, AppState, AppStatus, AuthStatus, DiagnosticReport, HubEvent, HubStatus, InstanceState,
    InstanceStatus, LastError, TaskLeaseStatus, TaskSelectionStatus,
};

use super::{HubShared, MAX_REPORTS, lock, unix_millis};

impl HubShared {
    // ------------------------------------------------------------------
    // 诊断（`/status`）
    // ------------------------------------------------------------------

    /// 记录某 App 最近一次错误（握手被拒、唤醒失败 / 超时）。
    pub(crate) fn record_app_error(&self, app_id: &str, code: Option<&str>, message: &str) {
        lock(&self.diagnostics).last_errors.insert(
            app_id.to_owned(),
            LastError { code: code.map(str::to_owned), message: message.to_owned(), at_ms: unix_millis() },
        );
    }

    /// 记录 SDK 的 `app/diagnostic` 上报，并发 [`HubEvent::AppDiagnostic`]。
    pub(crate) fn record_report(&self, report: DiagnosticReport) {
        self.emit(HubEvent::AppDiagnostic {
            app_id: report.app_id.clone(),
            instance_id: report.instance_id.clone(),
            code: report.code.clone(),
            message: report.message.clone(),
            count: report.count,
        });
        let mut d = lock(&self.diagnostics);
        if d.reports.len() >= MAX_REPORTS {
            d.reports.pop_front();
        }
        d.reports.push_back(report);
    }

    /// 当前运行状态（[`Hub::status`]、`GET /status`）。
    pub(crate) fn status(&self) -> HubStatus {
        let endpoints = self.endpoints.get();
        let waking: Vec<(String, Option<String>)> = {
            let now = tokio::time::Instant::now();
            lock(&self.wakes)
                .iter()
                .filter(|w| w.is_active(now))
                .map(|w| w.target())
                .collect()
        };
        let (last_errors, reports) = {
            let d = lock(&self.diagnostics);
            (d.last_errors.clone(), d.reports.iter().cloned().collect())
        };
        let infos = self.registry().app_infos(&HashMap::new());
        let now = tokio::time::Instant::now();
        let leased = self.leased_connections(now);
        let mut declarations = self.tool_declarations();
        let limit_counts = |app_id: &str| lock(&self.rates).counters(app_id);
        let mut apps: Vec<AppStatus> = infos
            .into_iter()
            .map(|a| {
                let power = |inst: &str, connected: bool| {
                    let mut p = lock(&self.power).instance(&a.app_id, inst, now)?;
                    if connected {
                        p.awake_reasons = self.awake_reasons(&a.app_id, inst, p.lifecycle_mode, &leased);
                    }
                    Some(p)
                };
                let is_waking = |inst: Option<&str>| {
                    waking.iter().any(|(app, i)| *app == a.app_id && (i.is_none() || i.as_deref() == inst))
                };
                let mut instances: Vec<InstanceStatus> = a
                    .instances
                    .into_iter()
                    .map(|info| InstanceStatus {
                        power: power(&info.instance_id, true),
                        info,
                        state: InstanceState::Connected,
                    })
                    .collect();
                instances.extend(a.dormant_instances.into_iter().map(|info| {
                    let state = if is_waking(Some(&info.instance_id)) {
                        InstanceState::Waking
                    } else {
                        InstanceState::Dormant
                    };
                    InstanceStatus { power: power(&info.instance_id, false), info, state }
                }));
                let state = if a.connected {
                    AppState::Connected
                } else if waking.iter().any(|(app, _)| *app == a.app_id) {
                    AppState::Waking
                } else if instances.is_empty() {
                    AppState::Disconnected
                } else {
                    AppState::Dormant
                };
                let counts = limit_counts(&a.app_id);
                AppStatus {
                    last_error: last_errors.get(&a.app_id).cloned(),
                    wakes: lock(&self.power).app_wakes(&a.app_id),
                    rate_limited: counts.rate_limited,
                    too_large: counts.too_large,
                    tools: declarations.remove(&a.app_id).unwrap_or_default(),
                    app_id: a.app_id,
                    name: a.name,
                    kind: AppKind::App,
                    state,
                    instances,
                }
            })
            .collect();
        apps.extend(lock(&self.upstreams).iter().map(|(name, st)| AppStatus {
            app_id: name.clone(),
            name: st.server_name.clone().unwrap_or_else(|| name.clone()),
            kind: AppKind::Upstream,
            state: if st.connected() { AppState::Connected } else { AppState::Disconnected },
            instances: Vec::new(),
            last_error: st.last_error.as_ref().map(|m| LastError { code: None, message: m.clone(), at_ms: 0 }),
            wakes: 0,
            rate_limited: limit_counts(name).rate_limited,
            too_large: limit_counts(name).too_large,
            tools: declarations.remove(name).unwrap_or_default(),
        }));
        // 只出现过错误（如握手被拒）、从未登记的 App 也列出，便于诊断。
        for (app_id, err) in &last_errors {
            if !apps.iter().any(|a| &a.app_id == app_id) {
                apps.push(AppStatus {
                    app_id: app_id.clone(),
                    name: app_id.clone(),
                    kind: AppKind::App,
                    state: AppState::Disconnected,
                    instances: Vec::new(),
                    last_error: Some(err.clone()),
                    wakes: lock(&self.power).app_wakes(app_id),
                    rate_limited: limit_counts(app_id).rate_limited,
                    too_large: limit_counts(app_id).too_large,
                    tools: Vec::new(),
                });
            }
        }
        apps.sort_by(|a, b| a.app_id.cmp(&b.app_id));
        // 在结构体字面量之外取：字面量中 `lock(&self.leases)` 的守卫活到语句结束，task_statuses 再锁会自锁。
        let tasks = self.task_statuses();
        HubStatus {
            identity: self.identity.clone(),
            listen: endpoints.and_then(|e| e.listen.clone()),
            ipc_endpoint: endpoints.and_then(|e| e.ipc_endpoint.clone()),
            started_at_ms: endpoints.map(|e| e.started_at_ms).unwrap_or(0),
            mcp_http: self.config.mcp_http,
            auth: AuthStatus {
                token_configured: self.config.http.token.is_some(),
                token_required_without_origin: self.config.http.token.is_some()
                    && self.config.http.require_token_without_origin,
            },
            mcp_sessions: self.mcp_session_count(),
            mcp_listen_streams: Some(lock(&self.subscribers).listen_count()),
            apps,
            reports,
            lease: Some(lock(&self.leases).status(&self.config.lease, self.config.lease_ttl)),
            limits: Some(LimitOverrides::from_policy(&self.config.limits)),
            output_validation: Some(self.config.output_validation),
            policy: Some(lock(&self.policy).status()),
            dormant_store: self.persist.as_ref().map(crate::lifecycle::Persist::status),
            tasks: Some(tasks),
            agents: Some(lock(&self.agents).names()),
            usage: Some(lock(&self.usage).status()),
            locks: Some(self.lock_status()),
            calls: Some(self.call_statuses()),
        }
    }

    /// 各 App（含上游）的工具声明（`/status` 的 `tools`）。
    fn tool_declarations(&self) -> HashMap<String, Vec<crate::types::ToolDeclaration>> {
        let mut out: HashMap<String, Vec<crate::types::ToolDeclaration>> = HashMap::new();
        for t in self.registry().tools() {
            out.entry(t.app_id.clone()).or_default().push(call::tool_declaration(&t.info));
        }
        for (name, st) in lock(&self.upstreams).iter() {
            out.entry(name.clone()).or_default().extend(st.tools.iter().map(call::upstream_tool_declaration));
        }
        out
    }

    /// `/status` 的 `tasks`（只读快照，按调用方键排序）。
    pub(crate) fn task_statuses(&self) -> Vec<AgentTaskStatus> {
        let now = tokio::time::Instant::now();
        let ms = |d: Duration| u64::try_from(d.as_millis()).unwrap_or(u64::MAX);
        // 先取请求活动再锁任务表：与空闲回收（lifecycle::task_expiries）相同的加锁顺序，不同时持有两把锁。
        let keys: Vec<CallerKey> = lock(&self.agent_tasks).iter().map(|(k, _)| k.clone()).collect();
        let activity: HashMap<String, (u32, tokio::time::Instant)> = {
            let book = lock(&self.leases);
            keys.iter().filter_map(|k| book.activity(k.as_str()).map(|a| (k.as_str().to_owned(), a))).collect()
        };
        let tasks = lock(&self.agent_tasks);
        let mut out: Vec<AgentTaskStatus> = tasks
            .iter()
            .map(|(key, t)| {
                let mut selections: Vec<TaskSelectionStatus> = t
                    .live_selections(self.selection_ttl(key), now)
                    .map(|(app_id, instance_id, until)| TaskSelectionStatus {
                        app_id: app_id.to_owned(),
                        instance_id: instance_id.to_owned(),
                        expires_in_ms: until.map(|u| ms(u.saturating_duration_since(now))),
                    })
                    .collect();
                selections.sort_by(|a, b| a.app_id.cmp(&b.app_id));
                let mut leases: Vec<TaskLeaseStatus> = t
                    .leases
                    .values()
                    .filter_map(|l| {
                        let until = l.expires().filter(|u| *u > now)?;
                        Some(TaskLeaseStatus {
                            connection_id: l.conn.upgrade()?.cid.clone(),
                            expires_in_ms: ms(until.saturating_duration_since(now)),
                        })
                    })
                    .collect();
                leases.sort_by(|a, b| a.connection_id.cmp(&b.connection_id));
                let (inflight, idle_ms) = match activity.get(key.as_str()) {
                    Some((n, last)) => (*n, Some(ms(now.saturating_duration_since(*last)))),
                    None => (0, None),
                };
                AgentTaskStatus {
                    id: t.id.clone(),
                    caller: key.to_string(),
                    kind: key.kind(),
                    agent: key.agent().map(|a| a.as_str().to_owned()),
                    selections,
                    leases,
                    inflight,
                    idle_ms,
                }
            })
            .collect();
        out.sort_by(|a, b| a.caller.cmp(&b.caller));
        out
    }
}
