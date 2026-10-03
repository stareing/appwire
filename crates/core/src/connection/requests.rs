//! 连接状态机：Host 发来的调用、资源、导航请求。

use super::*;

impl Client {
    // ---- 调用 -----------------------------------------------------------

    /// 在并发上限内按到达顺序启动排队中的调用（spec/protocol.md 5.3）：因本工具的 `concurrency` 或互斥组正忙而不能开始的调用
    /// 留在原位，后面能开始的先开始（不被队头阻塞）；它们在占用者结束后的下一轮按原顺序优先。
    pub(crate) fn pump_calls(&mut self) {
        let max = self.config.max_concurrent_calls.max(1);
        let mut index = 0;
        while self.calls.running_len() < max {
            let Some(tool) = self.calls.queued_at(index).map(|c| c.tool) else { break };
            let verdict = match self.registry.tool(tool) {
                None => Err((ErrorKind::ToolNotFound, "在排队期间已被注销。请重新获取工具列表。")),
                Some(def) if !def.enabled => Err((ErrorKind::ToolDisabled, "在排队期间已被禁用，当前不可用。")),
                Some(def) if self.busy_blocks(def) => Ok(None),
                Some(def) => Ok(self.calls.can_start(tool, def.concurrency, def.exclusive.as_deref()).then(|| def.exclusive.clone())),
            };
            match verdict {
                Ok(None) => index += 1,
                Ok(Some(exclusive)) => {
                    let Some(mut call) = self.calls.remove_queued(index) else { break };
                    call.exclusive = exclusive;
                    // @why 参数移交给 handler 而不深拷贝：开始执行后核心不再读取参数（1 MiB 对象数组省约 3 ms / 13 万次分配）。
                    let arguments = std::mem::take(&mut call.arguments);
                    self.events.push_back(Event::InvokeTool {
                        call_id: call.call_id.clone(),
                        tool: call.tool,
                        name: call.name.clone(),
                        arguments,
                        idempotency_key: call.idempotency_key.clone(),
                    });
                    self.calls.start(call);
                }
                Err((kind, why)) => {
                    let Some(call) = self.calls.remove_queued(index) else { break };
                    let err = tool_error(kind, format!("工具 {} {why}", call.name));
                    self.respond_call(call, Err(err), false);
                }
            }
        }
    }

    /// 用户正在操作（[`Client::set_busy`]）时 `def` 的调用不能开始：写工具（生效注解不是 `readOnlyHint: true`）。
    fn busy_blocks(&self, def: &ToolDef) -> bool {
        self.busy && ToolAnnotations::effective(def.risk, def.annotations.as_ref()).read_only_hint != Some(true)
    }

    /// 用户正在操作且策略为拒绝时，以 `RATE_LIMITED`（`details.scope = "busy"`）拒绝排队中的写调用（未开始，不进去重表）；
    /// 之后按队列重新调度。
    pub(crate) fn settle_busy(&mut self) {
        if self.config.busy_policy == BusyPolicy::Reject {
            let mut index = 0;
            while let Some(tool) = self.calls.queued_at(index).map(|c| c.tool) {
                if !self.registry.tool(tool).is_some_and(|def| self.busy_blocks(def)) {
                    index += 1;
                    continue;
                }
                let Some(call) = self.calls.remove_queued(index) else { break };
                let err = busy_error(&format!("工具 {} 未执行", call.name)).into();
                self.respond_call(call, Err(err), false);
            }
        }
        self.settle_navigations();
        self.pump_calls();
    }

    /// 推迟的导航：用户结束操作后按到达顺序执行；改为拒绝策略时随即拒绝。
    fn settle_navigations(&mut self) {
        if self.busy && self.config.busy_policy == BusyPolicy::Queue {
            return;
        }
        for d in std::mem::take(&mut self.session.deferred_navigations) {
            if self.busy {
                self.respond(d.request_id, Err(busy_error(&format!("未切换到页面「{}」", d.params.page)).into()));
            } else {
                self.start_navigate(d.request_id, d.params);
            }
        }
    }

    /// 推迟期间 Host 已不再等待的导航：回复 `NAVIGATION_FAILED`（`reason: "timeout"`），不再执行。
    pub(super) fn expire_navigations(&mut self, now: Millis) {
        let (expired, kept) =
            std::mem::take(&mut self.session.deferred_navigations).into_iter().partition(|d| d.deadline <= now);
        self.session.deferred_navigations = kept;
        for d in expired {
            let err = ToolError::navigation_failed(
                format!("用户一直在操作 App，未切换到页面「{}」。请稍后重试，或先告知用户。", d.params.page),
                proto::navigation_reason::TIMEOUT,
            );
            self.respond(d.request_id, Err(err.into()));
        }
    }

    /// 刚到达的调用 `call_id` 仍在排队且队列超过 `maxQueuedCalls` → 以 `RATE_LIMITED` 拒绝一个排队中的调用（未开始，不进去重表）：
    /// 有优先级低于新调用的，拒绝其中最后到达的一个（低优先级让路，第 16 项 P6）；否则拒绝新调用。
    fn reject_overflow(&mut self, call_id: &str) {
        let max = self.config.max_queued_calls;
        if max == 0 || self.calls.queued_len() <= max {
            return;
        }
        let Some(priority) = self.calls.queued_priority(call_id) else { return };
        let victim = self.calls.lowest_below(priority).map_or_else(|| call_id.to_owned(), |c| c.call_id.clone());
        let preempted = victim != call_id;
        let Some(call) = self.calls.take_queued(&victim) else { return };
        let why = if preempted { "为更高优先级的调用让路，" } else { "" };
        let mut details = serde_json::json!({ "scope": "queue", "limit": max });
        if preempted {
            details["preempted"] = Value::Bool(true);
        }
        let err = ToolError::new(
            ErrorKind::RateLimited,
            format!("App 正忙：排队中的调用已达上限（{max} 个），工具 {} {why}未执行。请稍后重试。", call.name),
        )
        .with_details(details)
        .into();
        self.respond_call(call, Err(err), false);
    }

    pub(crate) fn finish_call(&mut self, call: Call, outcome: Result<CallOutput, ToolError>) {
        // @why 直接序列化为文本：经 `to_value` 再 `to_json` 会深拷贝两次（1 MiB 对象数组约 20 ms / 36 万次分配）。
        let outcome = match outcome {
            Ok(out) => Ok(invoke_result_json(&out)),
            Err(e) => Err(RpcError::from(e)),
        };
        self.respond_call(call, outcome, true);
        self.pump_calls();
    }

    pub(super) fn on_invoke(&mut self, id: RequestId, p: ToolsInvokeParams, now: Millis) {
        self.session.served_call = true;
        // 去重（spec/protocol.md 3.3）：已开始执行过的 callId（或同一工具的同一幂等键）重放首次结果；
        // 进行中 / 排队中的挂到同一次执行上。
        // @why 命中只记一条警告日志（SDK 本地可观测，spec/protocol.md 3.3），不另设计数器或上报 Host。
        let idem = p.idempotency_key.as_deref().map(|k| crate::dedup::idempotency_match_key(&p.name, k));
        if let Some(outcome) = self.dedup.lookup(&self.config.call_dedup, &p.call_id, idem.as_deref(), now) {
            self.warn(format!("callId {:?} 重复到达（调用去重）：重放首次结果，不再执行", p.call_id));
            self.respond_outcome(id, &outcome);
            return;
        }
        if self.calls.contains(&p.call_id) {
            if self.config.call_dedup.enabled() {
                self.warn(format!("callId {:?} 重复到达（调用去重）：挂到执行中的同一次调用", p.call_id));
                self.calls.attach(&p.call_id, id);
            } else {
                let err = RpcError::invalid_params(format!("callId {:?} 已存在", p.call_id));
                self.respond(id, Err(err));
            }
            return;
        }
        if self.config.call_dedup.enabled()
            && let Some(key) = p.idempotency_key.as_deref()
            && self.calls.attach_idempotent(&p.name, key, id.clone())
        {
            self.warn(format!("callId {:?} 的幂等键与执行中的调用相同（调用去重）：挂到同一次调用", p.call_id));
            return;
        }
        let tool = match self.registry.tool_by_name(&p.name) {
            None => Err(tool_error(
                ErrorKind::ToolNotFound,
                format!("工具 {} 不存在（可能所在页面尚未加载或已离开）。请重新获取工具列表。", p.name),
            )),
            Some((_, def)) if !def.enabled => Err(tool_error(
                ErrorKind::ToolDisabled,
                format!("工具 {} 当前被禁用，App 当前状态下不可用。", p.name),
            )),
            Some((tool, _)) => Ok(tool),
        };
        let tool = match tool {
            Ok(t) => t,
            Err(e) => {
                self.respond(id, Err(e));
                return;
            }
        };
        let call_id = p.call_id.clone();
        self.calls.enqueue(Call {
            call_id: p.call_id,
            request_id: id,
            tool,
            name: p.name,
            arguments: p.arguments,
            idempotency_key: p.idempotency_key,
            timeout_ms: p.timeout_ms,
            deadline: p.timeout_ms.map(|t| now.saturating_add(t)),
            waiters: Vec::new(),
            exclusive: None,
            priority: p.priority,
        });
        self.settle_busy();
        self.reject_overflow(&call_id);
    }

    pub(super) fn on_cancel(&mut self, p: ToolsCancelParams) {
        let reason = p.reason.as_deref().map(|r| format!("：{r}")).unwrap_or_default();
        let (call, started) = if let Some(call) = self.calls.take_running(&p.call_id) {
            self.events.push_back(Event::CancelTool { call_id: call.call_id.clone(), reason: CancelReason::Requested });
            (call, true)
        } else if let Some(call) = self.calls.take_queued(&p.call_id) {
            (call, false)
        } else {
            self.warn(format!("tools/cancel 指向未知或已结束的调用 {:?}", p.call_id));
            return;
        };
        let err = tool_error(ErrorKind::Cancelled, format!("工具 {} 的调用已被取消{reason}", call.name));
        self.respond_call(call, Err(err), started);
        self.pump_calls();
    }

    // ---- 资源 -----------------------------------------------------------

    pub(super) fn resource_not_found(name: &str) -> RpcError {
        tool_error(ErrorKind::ResourceNotFound, format!("资源 {name} 不存在。请重新获取资源列表。"))
    }

    pub(super) fn on_read(&mut self, id: RequestId, p: ResourcesReadParams) {
        self.session.served_call = true;
        let Some((resource, def)) = self.registry.resource_by_name(&p.name) else {
            self.respond(id, Err(Self::resource_not_found(&p.name)));
            return;
        };
        let mime_type = def.mime_type.clone();
        self.next_read_id += 1;
        let read = ReadId(self.next_read_id);
        self.session.reads.insert(read, PendingRead { request_id: id, mime_type });
        self.events.push_back(Event::ReadResource { read, resource, name: p.name });
    }

    pub(crate) fn finish_read(&mut self, pending: PendingRead, outcome: Result<Value, ToolError>) {
        let outcome = match outcome {
            Ok(contents) => Ok(read_result_json(&contents, pending.mime_type.as_deref())),
            Err(e) => Err(RpcError::from(e)),
        };
        self.respond_outcome(pending.request_id, &outcome);
    }

    // ---- 导航（spec/protocol.md 3.4）------------------------------------

    pub(super) fn on_navigate(&mut self, id: RequestId, p: NavigateParams, now: Millis) {
        if !self.config.navigation {
            let err = ToolError::navigation_failed(
                format!("App 不支持由 Agent 导航（页面「{}」），请让用户自行打开该页面。", p.page),
                proto::navigation_reason::UNSUPPORTED,
            );
            self.respond(id, Err(err.into()));
            return;
        }
        if !proto::is_valid_name(&p.page) {
            self.respond(id, Err(RpcError::invalid_params(format!("app/navigate 的页面名不合法：{:?}", p.page))));
            return;
        }
        // 用户正在操作：导航会切走用户正在看的界面，与写调用同样按 busyPolicy 处理；Host 没给等待时限（旧 Host）时不推迟。
        if self.busy {
            match p.timeout_ms {
                Some(t) if self.config.busy_policy == BusyPolicy::Queue => {
                    let deadline = now.saturating_add(t);
                    self.session.deferred_navigations.push(DeferredNavigate { request_id: id, params: p, deadline });
                }
                _ => self.respond(id, Err(busy_error(&format!("未切换到页面「{}」", p.page)).into())),
            }
            return;
        }
        self.start_navigate(id, p);
    }

    /// 交给导航回调（实例不可见且不能后台导航时回复 `USER_ACTION_REQUIRED`）。
    fn start_navigate(&mut self, id: RequestId, p: NavigateParams) {
        if self.visibility != Visibility::Visible && !self.config.navigate_in_background {
            self.respond(id, Err(foreground_required(&p.page).into()));
            return;
        }
        self.session.served_call = true;
        self.next_navigate_id += 1;
        let navigate = NavigateId(self.next_navigate_id);
        self.session.navigations.insert(navigate, id);
        self.events.push_back(Event::Navigate { navigate, page: p.page, params: p.params.unwrap_or(Value::Null) });
    }

    pub(crate) fn finish_navigate(&mut self, request_id: RequestId, outcome: Result<(), ToolError>) {
        let outcome = outcome.map(|()| to_value(&NavigateResult { ok: true })).map_err(RpcError::from);
        self.respond(request_id, outcome);
    }

    pub(super) fn on_subscribe(&mut self, id: RequestId, p: ResourceSubscribeParams, subscribe: bool, now: Millis) {
        if self.registry.resource_by_name(&p.name).is_none() {
            self.respond(id, Err(Self::resource_not_found(&p.name)));
            return;
        }
        let changed_while_away = self.life.carried_subscriptions.remove(&p.name).is_some_and(|s| s.pending);
        if !subscribe {
            self.session.subscriptions.remove(&p.name);
            self.respond(id, Ok(json!({})));
            return;
        }
        self.session.subscriptions.get_or_insert_default(p.name.clone());
        self.respond(id, Ok(json!({})));
        // 未连接期间的变化：Host 重新订阅后补发（spec/lifecycle.md 第 13 节 B3）。
        if changed_while_away {
            self.send_resource_updated(p.name, now);
        }
    }

    pub(super) fn send_resource_updated(&mut self, name: String, now: Millis) {
        if let Some(sub) = self.session.subscriptions.get_mut(&name) {
            sub.last_sent = Some(now);
            sub.pending = false;
            self.notify(method::RESOURCES_UPDATED, &ResourceUpdatedParams { name });
        }
    }

    pub(crate) fn on_resource_changed(&mut self, resource: ResourceId, now: Millis) {
        let Some(def) = self.registry.resource(resource) else { return };
        let (name, realtime) = (def.name.clone(), def.realtime);
        if !self.connected() || !self.session.subscriptions.contains_key(&name) {
            self.on_carried_resource_changed(&name, realtime, now);
            return;
        }
        let throttle = self.config.resource_update_throttle_ms;
        let Some(sub) = self.session.subscriptions.get_mut(&name) else { return };
        match sub.last_sent {
            Some(t) if now < t.saturating_add(throttle) => sub.pending = true,
            _ => self.send_resource_updated(name, now),
        }
    }
}

/// 用户正在操作（[`Client::set_busy`]）时拒绝写调用 / 导航：`RATE_LIMITED`（`details.scope = "busy"`），`what` 说明未做的事。
fn busy_error(what: &str) -> ToolError {
    ToolError::new(ErrorKind::RateLimited, format!("用户正在操作 App，写操作暂不执行：{what}。请稍后重试，或先告知用户。"))
        .with_details(serde_json::json!({ "scope": "busy" }))
}

/// 实例不可见、且不能自行回到前台时对 `app/navigate` 的回复（spec/protocol.md 3.4）：`USER_ACTION_REQUIRED`（`reason: "foreground"`）。
fn foreground_required(page: &str) -> ToolError {
    ToolError::user_action_required(
        format!("App 当前在后台，无法自行切换到页面「{page}」。请让用户打开 App 后重试。"),
        Some(proto::user_action_reason::FOREGROUND),
        None,
    )
}
