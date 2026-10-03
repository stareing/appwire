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

    /// 刚到达的调用 `call_id` 仍在排队且队列超过 `maxQueuedCalls` → 移出并以 `RATE_LIMITED` 拒绝（未开始，不进去重表）。
    fn reject_overflow(&mut self, call_id: &str) {
        let max = self.config.max_queued_calls;
        if max == 0 || self.calls.queued_len() <= max {
            return;
        }
        let Some(call) = self.calls.take_queued(call_id) else { return };
        let err = ToolError::new(
            ErrorKind::RateLimited,
            format!("App 正忙：排队中的调用已达上限（{max} 个），工具 {} 未执行。请稍后重试。", call.name),
        )
        .with_details(serde_json::json!({ "scope": "queue", "limit": max }))
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
        });
        self.pump_calls();
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

    pub(super) fn on_navigate(&mut self, id: RequestId, p: NavigateParams) {
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

/// 实例不可见、且不能自行回到前台时对 `app/navigate` 的回复（spec/protocol.md 3.4）：`USER_ACTION_REQUIRED`（`reason: "foreground"`）。
fn foreground_required(page: &str) -> ToolError {
    ToolError::user_action_required(
        format!("App 当前在后台，无法自行切换到页面「{page}」。请让用户打开 App 后重试。"),
        Some(proto::user_action_reason::FOREGROUND),
        None,
    )
}
