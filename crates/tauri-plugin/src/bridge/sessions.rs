//! [`Sessions`]：全部 WebView 会话的登记、导航目标与状态广播。

use super::*;

impl Sessions {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(super) fn get_or_create(
        &self,
        label: &str,
        create: impl FnOnce() -> Result<Session, OpError>,
    ) -> Result<Arc<Session>, OpError> {
        let mut map = lock(&self.map);
        if let Some(session) = map.get(label) {
            return Ok(session.clone());
        }
        let session = Arc::new(create()?);
        map.insert(label.to_owned(), session.clone());
        Ok(session)
    }

    /// 当前有登记的 WebView 数量。
    pub(crate) fn count(&self) -> usize {
        lock(&self.map).len()
    }

    pub(crate) fn attach_client(&self, client: NativeClient) {
        *lock(&self.client) = Some(client);
    }

    pub(crate) fn detach_client(&self) {
        let client = lock(&self.client).take();
        drop(client);
    }

    /// 客户端当前的连接 ID；未连接、旧 Host 或未关联客户端时为 `None`。
    fn connection_id(&self) -> Option<String> {
        lock(&self.client).as_ref().and_then(NativeClient::connection_id)
    }

    /// 注销该 WebView 的全部登记。
    pub(crate) fn end(&self, label: &str) {
        let removed = lock(&self.map).remove(label);
        if let Some(session) = removed {
            self.release_events(session.dispose());
        }
        self.sync_busy();
    }

    /// 把各页 `busy.set` 之或同步给客户端（汇总值变化时）；页面注销后其声明随之失效。
    pub(super) fn sync_busy(&self) {
        use std::sync::atomic::Ordering;
        let sessions: Vec<Arc<Session>> = lock(&self.map).values().cloned().collect();
        let busy = sessions.iter().any(|s| s.busy());
        if self.pages_busy.swap(busy, Ordering::SeqCst) != busy
            && let Some(client) = lock(&self.client).as_ref()
        {
            client.set_busy(busy);
        }
    }

    /// 页面卸载（`reset`、页面开始加载）：注销登记，并结束该页的导航处理（进行中的导航失败）。
    pub(crate) fn end_page(&self, label: &str) {
        self.end(label);
        self.end_navigation(|p| p.label == label);
    }

    /// 注销该窗口内全部 WebView 的登记（窗口销毁）。
    pub(crate) fn end_window(&self, window: &str) {
        let removed: Vec<Arc<Session>> = {
            let mut map = lock(&self.map);
            let labels: Vec<String> = map
                .iter()
                .filter(|(_, s)| s.window == window)
                .map(|(label, _)| label.clone())
                .collect();
            labels
                .iter()
                .filter_map(|label| map.remove(label))
                .collect()
        };
        let events = removed.iter().flat_map(|s| s.dispose()).collect();
        self.release_events(events);
        self.sync_busy();
        self.end_navigation(|p| p.window == window);
    }

    /// 注销全部登记。
    pub(crate) fn end_all(&self) {
        let removed: Vec<Arc<Session>> = lock(&self.map).drain().map(|(_, s)| s).collect();
        let events = removed.iter().flat_map(|s| s.dispose()).collect();
        self.release_events(events);
        self.sync_busy();
        self.end_navigation(|_| true);
    }

    /// 开启 / 关闭本页的导航处理。
    ///
    /// @error 未调用 [`Bridge::enable_page_navigation`] 时开启返回 `NAVIGATION_DISABLED`。
    pub(super) fn set_navigation_target(
        &self,
        label: &str,
        window: &str,
        enabled: bool,
        sink: impl FnOnce() -> Arc<dyn PageSink>,
    ) -> Result<(), OpError> {
        if !enabled {
            let mut nav = lock(&self.navigation);
            if nav.target.as_ref().is_some_and(|t| t.page.label == label) {
                nav.target = None;
            }
            return Ok(());
        }
        if !self.navigation_enabled.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(op_error(
                "NAVIGATION_DISABLED",
                "插件未开启页面导航：Builder::page_navigation(true)",
            ));
        }
        let page = PageRef { label: label.to_owned(), window: window.to_owned() };
        lock(&self.navigation).target = Some(NavTarget { page, sink: sink() });
        Ok(())
    }

    /// Host 请求导航：送到目标页面，等待 `navigate.result`。在原生分发线程上调用。
    pub(super) fn forward_navigation(&self, request: NavigateHandle) {
        let (target, nav_id) = {
            let mut nav = lock(&self.navigation);
            let Some(target) = nav.target.clone() else {
                drop(nav);
                let _ = request.fail(&format!(
                    "没有页面处理导航（页面「{}」）：页面尚未加载或未开启导航",
                    request.page()
                ));
                return;
            };
            nav.next_id += 1;
            let nav_id = nav.next_id;
            nav.pending.insert(nav_id, (target.page.clone(), request.clone()));
            (target, nav_id)
        };
        let mut event = json!({ "type": "navigate", "navId": nav_id, "page": request.page() });
        if let Some(params) = request.params_json().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
            event["params"] = params;
        }
        target.sink.raise();
        if !target.sink.deliver(&event) {
            // 页面已不可达：结束其导航处理（含本次）。
            self.end_navigation(|p| p.label == target.page.label);
        }
    }

    /// 页面回复一次导航；未知或已结束的 navId 忽略。
    pub(super) fn finish_navigation(&self, nav_id: u64, ok: bool, failure: NavigationFailure<'_>) {
        let Some((_, request)) = lock(&self.navigation).pending.remove(&nav_id) else {
            return;
        };
        let message = failure.message.unwrap_or("页面导航失败");
        let detail = |key: &str| failure.details.and_then(|d| d.get(key)).and_then(Value::as_str);
        let _ = match (ok, failure.kind) {
            (true, _) => request.complete(),
            (false, Some("NAVIGATION_DENIED")) => request.deny(message),
            (false, Some("USER_ACTION_REQUIRED")) => request.fail_user_action(message, detail("reason"), detail("uri")),
            (false, _) => request.fail(message),
        };
    }

    /// 结束满足条件的 WebView 的导航处理：清除目标，进行中的导航以失败回复。
    fn end_navigation(&self, matches: impl Fn(&PageRef) -> bool) {
        let failed: Vec<NavigateHandle> = {
            let mut nav = lock(&self.navigation);
            if nav.target.as_ref().is_some_and(|t| matches(&t.page)) {
                nav.target = None;
            }
            let ids: Vec<u64> = nav.pending.iter().filter(|(_, (p, _))| matches(p)).map(|(id, _)| *id).collect();
            ids.iter().filter_map(|id| nav.pending.remove(id)).map(|(_, r)| r).collect()
        };
        for request in failed {
            let _ = request.fail("页面已关闭或刷新，导航未完成");
        }
    }

    pub(super) fn end_session(&self, session: &Arc<Session>) {
        let removed = {
            let mut map = lock(&self.map);
            match map.get(&session.label) {
                Some(current) if Arc::ptr_eq(current, session) => map.remove(&session.label),
                _ => None,
            }
        };
        let events = session.dispose();
        drop(removed);
        self.release_events(events);
        self.sync_busy();
        self.end_navigation(|p| p.label == session.label);
    }

    /// 把连接状态转发给全部页面。
    /// 连接 ID 在分发时读取：状态在此期间又变化时可能与 `state` 不一致，随后的状态事件会更正。
    pub(crate) fn broadcast_state(&self, state: &StateInfo) {
        let mut event = json!({ "type": "state", "state": state_json(state) });
        if let Some(cid) = self.connection_id() {
            event["connectionId"] = Value::String(cid);
        }
        let sessions: Vec<Arc<Session>> = lock(&self.map).values().cloned().collect();
        for session in sessions {
            session.send(&event);
        }
    }
}
