//! 生命周期：空闲判定、租约、`app/sleep` 握手、休眠 / 唤醒、唤醒参数解析（spec/lifecycle.md）。
//!
//! 与连接相关、断线即失效的状态放在 [`crate::connection::Session`]（空闲起点、租约、休眠握手）；
//! 跨连接保留的状态放在 [`Life`]（持有、恢复令牌、唤醒原因）。

use app_mcp_protocol as proto;
use proto::{RpcError, SleepParams, SleepReason, SleepResult, WakeReason, method};
use serde_json::Value;

use crate::connection::Outgoing;
use crate::vec_map::VecMap;
use crate::{CancelReason, Client, ConnectionState, Event, HoldId, LifecycleMode, Millis, Residency, Visibility};

/// App 显式休眠被拒绝且 Host 未给出 `retryAfterMs` 时的重试间隔。
const DEFAULT_SLEEP_RETRY_MS: Millis = 5_000;

/// 令牌最大长度（解码后）。
const MAX_TOKEN_LEN: usize = 512;

/// 跨连接保留的生命周期状态。
#[derive(Debug, Default)]
pub(crate) struct Life {
    /// 持有：id → 关联的调用（`hold_for_call`）。
    pub holds: VecMap<u64, Option<String>>,
    pub next_hold_id: u64,
    /// 上次休眠时 Host 返回的恢复令牌；成功握手后清除。
    pub resume_token: Option<String>,
    /// 下一次 `app/hello` 的 `wakeReason`；成功握手后清除。
    pub wake_reason: Option<WakeReason>,
    /// 尚未 `start` 时收到的唤醒（`start` 时据此连接）。
    pub pending_wake: Option<WakeReason>,
    /// 本进程由唤醒冷启动（配置带 launch token，或首次握手前收到唤醒）。
    pub launched_by_wake: bool,
    /// 曾经成功握手。
    pub ever_connected: bool,
    /// 空闲条件可能已变化但当时没有时间参数：`poll_timeout` 返回 `last_now`，请驱动层立即 `handle_timeout`。
    pub idle_recheck: bool,
    /// 最近一次输入的时间。
    pub last_now: Millis,
}

impl Life {
    pub fn after_handshake(&mut self) {
        self.resume_token = None;
        self.wake_reason = None;
        self.pending_wake = None;
        self.ever_connected = true;
    }
}

// ---------------------------------------------------------------------------
// 唤醒参数解析
// ---------------------------------------------------------------------------

/// 从操作系统激活参数 / URL 中提取唤醒令牌。`args` 可以是单个参数、完整命令行（空白分隔，允许引号）或 URL。
///
/// 识别的形式：
/// - `app-mcp-wake:<token>`（Windows AUMID 激活参数、Android extra）；
/// - `<scheme>://app-mcp/wake?token=<token>` 与 `<scheme>:app-mcp/wake?token=<token>`（自定义 URL scheme）；
/// - 任意 URL 的片段参数 `#app-mcp-wake=<token>`（Web）。
///
/// 令牌经百分号解码后必须是 1–512 个 `[A-Za-z0-9._~-]` 字符；其他输入返回 `None`。
pub fn parse_wake_token(args: &str) -> Option<String> {
    args.split_whitespace().map(|p| p.trim_matches(|c| c == '"' || c == '\'')).find_map(token_from_part)
}

fn token_from_part(part: &str) -> Option<String> {
    if let Some(rest) = part.strip_prefix("app-mcp-wake:") {
        return valid_token(rest);
    }
    if let Some(t) = token_from_wake_uri(part) {
        return Some(t);
    }
    let (_, fragment) = part.split_once('#')?;
    fragment.split('&').find_map(|kv| kv.strip_prefix("app-mcp-wake=")).and_then(valid_token)
}

/// `<scheme>:[//]app-mcp/wake?token=<token>`。
fn token_from_wake_uri(part: &str) -> Option<String> {
    let (scheme, rest) = part.split_once(':')?;
    let mut chars = scheme.chars();
    let scheme_ok = chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    if !scheme_ok {
        return None;
    }
    let rest = rest.strip_prefix("//").unwrap_or(rest);
    let rest = rest.split_once('#').map_or(rest, |(before, _)| before);
    let (path, query) = rest.split_once('?')?;
    if path.trim_end_matches('/') != "app-mcp/wake" {
        return None;
    }
    query.split('&').find_map(|kv| kv.strip_prefix("token=")).and_then(valid_token)
}

fn valid_token(raw: &str) -> Option<String> {
    let token = percent_decode(raw)?;
    let ok = !token.is_empty()
        && token.len() <= MAX_TOKEN_LEN
        && token.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'~' | b'-'));
    ok.then_some(token)
}

fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes.get(i + 1..i + 3)?;
            let hex = std::str::from_utf8(hex).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

// ---------------------------------------------------------------------------
// Client：生命周期实现
// ---------------------------------------------------------------------------

impl Client {
    fn mode(&self) -> LifecycleMode {
        self.config.lifecycle.mode
    }

    /// `start()`：on-demand 且没有唤醒时进入 Dormant，否则连接。
    pub(crate) fn on_start(&mut self) {
        let woken = self.life.pending_wake.take();
        if self.mode() == LifecycleMode::OnDemand && woken.is_none() && self.launch_token.is_none() {
            self.set_state(ConnectionState::Dormant);
            return;
        }
        self.life.wake_reason = match woken {
            Some(reason) => Some(reason),
            None if self.mode() == LifecycleMode::Persistent => None,
            None if self.launch_token.is_some() => Some(WakeReason::OsActivation),
            None => Some(WakeReason::ColdStart),
        };
        self.events.push_back(Event::Connect);
        self.set_state(ConnectionState::Connecting);
    }

    /// Dormant → Waking。
    fn begin_wake(&mut self, reason: WakeReason) {
        self.life.wake_reason = Some(reason);
        self.events.push_back(Event::Connect);
        self.set_state(ConnectionState::Waking);
    }

    pub(crate) fn on_wake_token(&mut self, token: String, now: Millis) {
        // @why 已连接且不在休眠握手中：Host 按实例 ID 认领本连接（唤醒等待随回连结束），令牌已无用途；
        // 若保留，下次休眠被接受时会被当作"休眠中收到唤醒"而立即回连（spec/lifecycle.md 2 节）。
        if self.state == ConnectionState::Connected && self.session.sleeping.is_none() {
            self.restart_idle_timer(now);
            return;
        }
        self.launch_token = Some(token);
        if !self.life.ever_connected {
            self.life.launched_by_wake = true;
        }
        match self.state {
            ConnectionState::Idle => {
                self.life.pending_wake = Some(WakeReason::OsActivation);
                self.life.wake_reason = Some(WakeReason::OsActivation);
            }
            ConnectionState::Connecting | ConnectionState::Waking => {
                // hello 尚未发送，会携带新令牌。
                self.life.wake_reason = Some(WakeReason::OsActivation);
            }
            ConnectionState::Stopped | ConnectionState::Rejected { .. } => {
                self.warn(format!("当前状态 {:?} 下收到唤醒，已忽略", self.state));
            }
            ConnectionState::Handshaking | ConnectionState::PendingPairing => {}
            _ => {
                self.on_wake(WakeReason::OsActivation, now);
            }
        }
    }

    pub(crate) fn on_wake(&mut self, reason: WakeReason, now: Millis) -> bool {
        match self.state {
            ConnectionState::Dormant => {
                self.begin_wake(reason);
                true
            }
            ConnectionState::Backoff { .. } => {
                self.life.wake_reason = Some(reason);
                self.events.push_back(Event::Connect);
                self.set_state(ConnectionState::Connecting);
                true
            }
            ConnectionState::HostMismatch { .. } => {
                // 不自动重试；App 主动唤醒 / 连接时再试一次（如端点已改正、占用端口的程序已退出）。
                self.retry_count = 0;
                self.life.wake_reason = Some(reason);
                self.events.push_back(Event::Connect);
                self.set_state(ConnectionState::Connecting);
                true
            }
            ConnectionState::Connected if self.session.sleeping.is_some() => {
                self.session.rewake = true;
                true
            }
            ConnectionState::Connected => {
                self.restart_idle_timer(now);
                false
            }
            _ => false,
        }
    }

    pub(crate) fn on_sleep_requested(&mut self, reason: SleepReason, now: Millis) -> bool {
        match self.state {
            ConnectionState::Connected => {
                if self.session.sleeping.is_some() {
                    return false;
                }
                self.session.forced_sleep = Some(reason);
                self.begin_sleep(reason, now);
                true
            }
            ConnectionState::Backoff { .. } => {
                self.retry_count = 0;
                self.set_state(ConnectionState::Dormant);
                true
            }
            ConnectionState::Connecting
            | ConnectionState::Waking
            | ConnectionState::Handshaking
            | ConnectionState::PendingPairing => {
                // 尚未完成握手：Host 没有可保留的快照，直接断开。
                self.teardown(CancelReason::Disconnected, true);
                self.events.push_back(Event::Disconnect);
                self.retry_count = 0;
                self.set_state(ConnectionState::Dormant);
                true
            }
            _ => false,
        }
    }

    pub(crate) fn add_hold(&mut self, call: Option<String>, now: Millis) -> HoldId {
        self.life.next_hold_id += 1;
        let id = self.life.next_hold_id;
        self.life.holds.insert(id, call);
        if self.session.sleeping.is_some() {
            self.session.rewake = true;
        }
        self.refresh_idle(now);
        HoldId(id)
    }

    // ---- 空闲判定 -------------------------------------------------------

    /// 除租约外的空闲条件（spec/lifecycle.md 第 3 节）。
    fn idle_conditions_hold(&self) -> bool {
        self.calls.running_len() == 0
            && self.calls.queued_len() == 0
            && self.session.reads.is_empty()
            && self.session.subscriptions.is_empty()
            && self.life.holds.is_empty()
            && self.session.sleeping.is_none()
    }

    /// 根据当前条件维护空闲起点：条件不成立时清除，成立且尚无起点时从 `now` 开始计时。
    pub(crate) fn refresh_idle(&mut self, now: Millis) {
        if self.state != ConnectionState::Connected {
            return;
        }
        self.life.idle_recheck = false;
        if self.idle_conditions_hold() {
            if self.session.idle_anchor.is_none() {
                self.session.idle_anchor = Some(now);
            }
        } else {
            self.session.idle_anchor = None;
            if self.session.forced_sleep.is_none() {
                self.session.sleep_retry_at = None;
            }
        }
    }

    /// 重新开始空闲计时（可见性变化、租约变化、App 主动 wake）。
    pub(crate) fn restart_idle_timer(&mut self, now: Millis) {
        if self.session.idle_anchor.is_some() {
            self.session.idle_anchor = Some(now);
        }
        self.refresh_idle(now);
    }

    pub(crate) fn request_idle_recheck(&mut self) {
        if self.state == ConnectionState::Connected && self.mode() != LifecycleMode::Persistent {
            self.life.idle_recheck = true;
        }
    }

    fn idle_timeout_ms(&self) -> Millis {
        let p = &self.config.lifecycle;
        let base = match p.mode {
            LifecycleMode::OnDemand => p.grace_ms,
            _ => p.idle_timeout_ms,
        };
        match self.visibility {
            Visibility::Visible => base,
            Visibility::Hidden | Visibility::Frozen => base.min(p.hidden_idle_timeout_ms),
        }
    }

    /// 下一次发送 `app/sleep` 的时刻。
    pub(crate) fn sleep_deadline(&self) -> Option<Millis> {
        if self.state != ConnectionState::Connected || self.session.sleeping.is_some() {
            return None;
        }
        if self.session.forced_sleep.is_some() {
            return self.session.sleep_retry_at;
        }
        if self.mode() == LifecycleMode::Persistent || self.session.sleep_unsupported {
            return None;
        }
        let anchor = self.session.idle_anchor?;
        let lease = self.session.lease_until.unwrap_or(0);
        match self.session.sleep_retry_at {
            Some(retry) => Some(retry.max(lease)),
            None => Some(anchor.max(lease).saturating_add(self.idle_timeout_ms())),
        }
    }

    /// 计时到期触发的自动休眠原因（spec/protocol.md 8.5）：`on-demand` 为 `grace`，其余一律 `idle`。
    ///
    /// 可见性只影响计时长度（`hiddenIdleTimeoutMs`），不改变原因：`background` 专指"进入后台立即休眠"
    /// （bfcache、移动端进后台），由封装层以 `sleep_with_reason(Background)` 显式发起。
    fn auto_sleep_reason(&self) -> SleepReason {
        if self.mode() == LifecycleMode::OnDemand { SleepReason::Grace } else { SleepReason::Idle }
    }

    pub(crate) fn on_idle_timeout(&mut self, now: Millis) {
        if self.life.idle_recheck {
            self.refresh_idle(now);
        }
        if self.sleep_deadline().is_some_and(|d| now >= d) {
            let reason = self.session.forced_sleep.unwrap_or_else(|| self.auto_sleep_reason());
            self.begin_sleep(reason, now);
        }
    }

    pub(crate) fn on_lease(&mut self, ttl_ms: Millis, now: Millis) {
        self.session.lease_until = match ttl_ms {
            0 => None,
            ttl => Some(self.session.lease_until.unwrap_or(0).max(now.saturating_add(ttl))),
        };
        self.restart_idle_timer(now);
    }

    // ---- 休眠握手 -------------------------------------------------------

    /// 发送 `app/sleep`。之前排队的消息（调用结果、注册变更）都在它之前发出。
    fn begin_sleep(&mut self, reason: SleepReason, now: Millis) {
        let _ = now;
        self.flush_changes();
        let params = SleepParams {
            reason,
            wake: self.config.lifecycle.wake.clone(),
            tools_hash: self.tools_hash(),
        };
        let params = serde_json::to_value(&params).unwrap_or(Value::Null);
        self.request(method::SLEEP, params, Outgoing::Sleep);
        self.session.sleeping = Some(reason);
        self.session.sleep_retry_at = None;
        self.session.idle_anchor = None;
        self.session.rewake = false;
    }

    pub(crate) fn on_sleep_response(&mut self, outcome: Result<Value, RpcError>, now: Millis) {
        if self.session.sleeping.take().is_none() {
            self.warn("收到 app/sleep 的结果，但当前没有进行中的休眠请求，已忽略");
            return;
        }
        let result = match outcome {
            Ok(v) => serde_json::from_value::<SleepResult>(v),
            Err(e) => {
                self.warn(format!("app/sleep 失败（{}），本次连接内不再自动休眠", e.message));
                self.session.sleep_unsupported = true;
                self.session.forced_sleep = None;
                self.refresh_idle(now);
                return;
            }
        };
        match result {
            Ok(r) if r.accepted => self.enter_dormant(r.resume_token, now),
            Ok(r) => {
                if self.session.forced_sleep.is_some() {
                    let delay = r.retry_after_ms.unwrap_or(DEFAULT_SLEEP_RETRY_MS);
                    self.session.sleep_retry_at = Some(now.saturating_add(delay));
                    if self.session.rewake {
                        // 休眠期间 App 又要求保持连接：放弃显式休眠。
                        self.session.forced_sleep = None;
                        self.session.sleep_retry_at = None;
                    }
                } else {
                    self.session.sleep_retry_at = r.retry_after_ms.map(|ms| now.saturating_add(ms));
                }
                self.session.rewake = false;
                self.session.idle_anchor = None;
                self.refresh_idle(now);
            }
            Err(e) => {
                self.warn(format!("app/sleep 结果无效：{e}"));
                self.session.forced_sleep = None;
                self.refresh_idle(now);
            }
        }
    }

    /// Host 已接受休眠：关闭连接进入 Dormant。
    fn enter_dormant(&mut self, resume_token: Option<String>, now: Millis) {
        let _ = now;
        let rewake = self.session.rewake || self.launch_token.is_some();
        if self.calls.running_len() + self.calls.queued_len() > 0 {
            self.warn("Host 接受休眠时仍有进行中的调用，这些调用被取消");
        }
        if resume_token.is_none() {
            self.warn("Host 接受休眠但未返回 resumeToken，回连时将完整同步");
        }
        self.life.resume_token = resume_token;
        self.teardown(CancelReason::Disconnected, true);
        self.events.push_back(Event::Disconnect);
        self.retry_count = 0;
        self.set_state(ConnectionState::Dormant);
        if rewake {
            let reason =
                if self.launch_token.is_some() { WakeReason::OsActivation } else { WakeReason::App };
            self.begin_wake(reason);
            return;
        }
        let exit = match self.config.lifecycle.residency {
            Residency::Keep => false,
            Residency::ExitWhenIdle => self.life.launched_by_wake,
            Residency::ExitAlways => true,
        };
        if exit {
            self.events.push_back(Event::IdleExit);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wake_token_forms() {
        let ok = [
            ("app-mcp-wake:abc123", "abc123"),
            ("myapp://app-mcp/wake?token=tok_1", "tok_1"),
            ("myapp://app-mcp/wake/?x=1&token=tok-2&y=2", "tok-2"),
            ("ms-myapp:app-mcp/wake?token=T.3", "T.3"),
            ("https://example.com/shop?a=1#app-mcp-wake=w~4", "w~4"),
            ("http://localhost:5173/#foo=1&app-mcp-wake=w5", "w5"),
            ("C:\\app.exe --flag \"app-mcp-wake:quoted\"", "quoted"),
            ("app.exe   myapp://app-mcp/wake?token=a%2Db", "a-b"),
        ];
        for (input, token) in ok {
            assert_eq!(parse_wake_token(input).as_deref(), Some(token), "{input}");
        }
    }

    #[test]
    fn wake_token_rejects_other_input() {
        let long = format!("app-mcp-wake:{}", "a".repeat(513));
        let bad = [
            "",
            "   ",
            "--help",
            "app-mcp-wake:",
            "app-mcp-wake:has/slash",
            "app-mcp-wake:%zz",
            "app-mcp-wake:%E4%B8%AD",
            "myapp://other/wake?token=abc",
            "myapp://app-mcp/wake?tok=abc",
            "myapp://app-mcp/wake",
            "1app://app-mcp/wake?token=abc",
            "https://example.com/#other=1",
            "https://example.com/#app-mcp-wake=",
            "C:\\Users\\me\\file.txt",
            long.as_str(),
        ];
        for input in bad {
            assert_eq!(parse_wake_token(input), None, "{input}");
        }
    }
}
