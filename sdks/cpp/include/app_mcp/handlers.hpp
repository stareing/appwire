// app_mcp/handlers.hpp —— HoldGuard 与 Call / Read / Navigate 及回调分发（由 app_mcp.hpp 包含；直接包含 app_mcp.hpp 即可）。
#ifndef APP_MCP_HANDLERS_HPP
#define APP_MCP_HANDLERS_HPP

#include "types.hpp"

namespace app_mcp {

// ---------------------------------------------------------------------------
// HoldGuard
// ---------------------------------------------------------------------------

/// 阻止自动休眠的持有（Client::hold、Call::hold）。只能移动；析构或 release() 时释放。
class HoldGuard {
public:
    HoldGuard() = default;
    explicit HoldGuard(AmHold* h) : h_(h) {}
    HoldGuard(HoldGuard&& o) noexcept : h_(std::exchange(o.h_, nullptr)) {}
    HoldGuard& operator=(HoldGuard&& o) noexcept {
        if (this != &o) {
            release();
            h_ = std::exchange(o.h_, nullptr);
        }
        return *this;
    }
    HoldGuard(const HoldGuard&) = delete;
    HoldGuard& operator=(const HoldGuard&) = delete;
    ~HoldGuard() { release(); }

    /// 是否仍持有。
    bool active() const noexcept { return h_ != nullptr; }
    explicit operator bool() const noexcept { return active(); }
    /// 提前释放。幂等。
    void release() noexcept { am_hold_release(std::exchange(h_, nullptr)); }

private:
    AmHold* h_ = nullptr;
};

// ---------------------------------------------------------------------------
// BusyScope
// ---------------------------------------------------------------------------

namespace detail {

/// 合并 Client::set_busy 显式开关与 Client::busy() 作用域计数（spec/protocol.md 5.3「用户正在操作」）。
/// @invariant 交给原生库的值 = 开关 ∨ 作用域数 > 0；开关与作用域互不清除；只在有效值变化时调用 am_client_set_busy。
/// @invariant client 由 Client 拥有：Client 释放 / 移走句柄时置空，之后的变化只记账不下发。
struct BusyState {
    std::mutex mu;
    AmClient* client = nullptr;
    bool manual = false;
    uint64_t scopes = 0;
    bool applied = false;

    /// 在锁内调用；有效值变化时下发。@error 下发失败时返回状态码，applied 不变（下次变化重试）。
    AmStatus push() {
        bool effective = manual || scopes > 0;
        if (effective == applied || !client) return AM_OK;
        AmStatus s = am_client_set_busy(client, effective);
        if (s == AM_OK) applied = effective;
        return s;
    }
    bool effective() {
        std::lock_guard<std::mutex> lock(mu);
        return manual || scopes > 0;
    }
};

}  // namespace detail

/// 用户正在操作的作用域（Client::busy()）：构造时计数 +1，析构或 end() 时 -1。只能移动；可嵌套、可跨线程结束。
/// 有效 busy = Client::set_busy 显式开关 ∨ 未结束作用域数 > 0：set_busy(false) 不结束进行中的作用域，作用域结束也不清显式开关。
class BusyScope {
public:
    BusyScope() = default;
    explicit BusyScope(std::shared_ptr<detail::BusyState> state) : state_(std::move(state)) {}
    BusyScope(BusyScope&& o) noexcept : state_(std::move(o.state_)) {}
    BusyScope& operator=(BusyScope&& o) noexcept {
        if (this != &o) {
            end();
            state_ = std::move(o.state_);
        }
        return *this;
    }
    BusyScope(const BusyScope&) = delete;
    BusyScope& operator=(const BusyScope&) = delete;
    ~BusyScope() { end(); }

    /// 是否仍在作用域内（尚未结束）。
    bool active() const noexcept { return state_ != nullptr; }
    explicit operator bool() const noexcept { return active(); }
    /// 提前结束（计数 -1）。幂等。
    /// @error 下发失败（如客户端已停止）被忽略：析构路径不能抛出，且停止后的客户端不再执行调用。
    void end() noexcept {
        auto state = std::move(state_);
        state_ = nullptr;
        if (!state) return;
        try {
            std::lock_guard<std::mutex> lock(state->mu);
            --state->scopes;
            (void)state->push();
        } catch (...) {
        }
    }

private:
    std::shared_ptr<detail::BusyState> state_;
};

// ---------------------------------------------------------------------------
// Call / Read
// ---------------------------------------------------------------------------

/// 一次工具调用。只能移动；必须 complete 或 fail 一次，否则析构时以 HANDLER_ERROR 失败。
class Call {
public:
    explicit Call(std::shared_ptr<detail::Pending<AmCall>> state) : state_(std::move(state)) {
        if (AmCall* c = state_->raw.load()) {
            id_ = am_call_id(c);
            tool_name_ = am_call_tool_name(c);
            arguments_json_ = am_call_arguments_json(c);
            if (const char* key = am_call_idempotency_key(c)) idempotency_key_ = key;
        }
    }
    Call(Call&&) noexcept = default;
    Call& operator=(Call&& other) noexcept {
        if (this != &other) {
            finish_abandoned();
            state_ = std::move(other.state_);
            id_ = std::move(other.id_);
            tool_name_ = std::move(other.tool_name_);
            arguments_json_ = std::move(other.arguments_json_);
            idempotency_key_ = std::move(other.idempotency_key_);
        }
        return *this;
    }
    Call(const Call&) = delete;
    Call& operator=(const Call&) = delete;
    ~Call() { finish_abandoned(); }

    const std::string& id() const noexcept { return id_; }
    const std::string& tool_name() const noexcept { return tool_name_; }
    /// 参数（JSON 对象文本，已由 Host 按 inputSchema 校验）。
    const std::string& arguments_json() const noexcept { return arguments_json_; }
    /// Agent 给出的幂等键（app_mcp.h v16，spec/protocol.md 3.3，原样）；没有时为 nullopt。
    /// App 自行决定如何使用（如作为业务去重键）。
    const std::optional<std::string>& idempotency_key() const noexcept { return idempotency_key_; }

    /// 是否仍待完成。
    bool pending() const noexcept { return state_ && state_->raw.load() != nullptr; }
    explicit operator bool() const noexcept { return pending(); }

    bool is_cancelled() const {
        AmCall* c = state_ ? state_->raw.load() : nullptr;
        return c ? am_call_is_cancelled(c) : true;
    }

    /// 设置取消回调（在分发线程上调用；已取消时立即在当前线程调用一次）。
    void on_cancel(std::function<void(CancelReason)> f) {
        AmCall* c = state_ ? state_->raw.load() : nullptr;
        if (!c) throw Error(AM_ERR_ALREADY_COMPLETED, "调用已完成");
        using F = std::function<void(CancelReason)>;
        auto* holder = new F(std::move(f));
        detail::check(am_call_set_cancel_callback(
            c,
            [](void* ud, AmCancelReason r) {
                try {
                    (*static_cast<F*>(ud))(r);
                } catch (...) {
                }
            },
            holder, &detail::delete_fn<F>));
    }

    /// 成功完成。data_json 为 JSON 文本（默认 null）。非法 JSON 抛出 Error 且调用仍待完成。
    void complete(const std::string& data_json = "null", const std::vector<std::string>& state_hints = {}) {
        AmCall* c = take();
        std::vector<const char*> hints;
        hints.reserve(state_hints.size());
        for (const auto& h : state_hints) hints.push_back(h.c_str());
        AmStatus s = am_call_complete(c, data_json.c_str(), hints.empty() ? nullptr : hints.data(), hints.size());
        if (s == AM_ERR_INVALID_JSON) state_->put_back(c);
        detail::check(s);
    }

    /// 成功完成，附带业务状态、摘要与内容注解。data_json 非法时抛出 Error 且调用仍待完成。
    void complete(const CallResult& result) {
        std::vector<const char*> hints;
        hints.reserve(result.state_hints.size());
        for (const auto& h : result.state_hints) hints.push_back(h.c_str());
        std::optional<std::string> annotations;
        if (result.annotations) annotations = detail::to_json(*result.annotations);
        AmCallResult r{};
        r.struct_size = sizeof(AmCallResult);
        r.data_json = detail::c_str_or_null(result.data_json);
        r.state_hints = hints.empty() ? nullptr : hints.data();
        r.state_hints_len = hints.size();
        r.status = result.status;
        r.state_resource = detail::c_str_or_null(result.state_resource);
        r.summary = detail::c_str_or_null(result.summary);
        r.annotations_json = detail::c_str_or_null(annotations);
        AmCall* c = take();
        AmStatus s = am_call_complete_ex(c, &r);
        if (s == AM_ERR_INVALID_JSON) state_->put_back(c);
        detail::check(s);
    }

    /// 失败完成。kind 为协议错误类别字符串，如 "HANDLER_ERROR"、"USER_REJECTED"。
    void fail(const std::string& kind, const std::string& message) {
        AmCall* c = take();
        detail::check(am_call_fail(c, kind.c_str(), message.c_str()));
    }

    /// 失败完成并附带结构化详情（JSON 文本）。非法 JSON 抛出 Error 且调用仍待完成。
    void fail(const std::string& kind, const std::string& message, const std::string& details_json) {
        AmCall* c = take();
        AmStatus s = am_call_fail_with_details(c, kind.c_str(), message.c_str(), details_json.c_str());
        if (s == AM_ERR_INVALID_JSON) state_->put_back(c);
        detail::check(s);
    }

    /// 以 USER_ACTION_REQUIRED 失败完成（app_mcp.h v11）：需要用户本人操作后才能继续。
    /// reason（见 user_action_reason）与 uri 缺省时不出现在错误的 data 中。
    void fail_user_action(const std::string& message, const std::optional<std::string>& reason = std::nullopt,
                          const std::optional<std::string>& uri = std::nullopt) {
        AmCall* c = take();
        detail::check(am_call_fail_user_action(c, message.c_str(), detail::c_str_or_null(reason),
                                               detail::c_str_or_null(uri)));
    }

    /// 延长持有：调用完成后仍阻止自动休眠（handler 发起的长任务），直到 HoldGuard 释放。
    /// 必须在完成调用之前取得。
    HoldGuard hold() const {
        AmCall* c = state_ ? state_->raw.load() : nullptr;
        if (!c) throw Error(AM_ERR_ALREADY_COMPLETED, "调用已完成");
        AmHold* h = nullptr;
        detail::check(am_call_hold(c, &h));
        return HoldGuard(h);
    }

    /// 报告进度（app_mcp.h v10，spec/protocol.md 3.3）：Host 合并后转发给 Agent。progress 应递增，total 未知时省略。
    /// 必须在完成调用之前、在完成调用的同一线程上调用；调用已完成或已取消、未连接时无副作用（不抛出）。
    void progress(double progress, std::optional<double> total = std::nullopt,
                  const std::optional<std::string>& message = std::nullopt) const noexcept {
        AmCall* c = state_ ? state_->raw.load() : nullptr;
        if (!c) return;
        am_call_progress(c, progress, total.value_or(-1.0), message ? message->c_str() : nullptr);
    }

private:
    AmCall* take() {
        AmCall* c = state_ ? state_->take() : nullptr;
        if (!c) throw Error(AM_ERR_ALREADY_COMPLETED, "调用已完成");
        return c;
    }

    void finish_abandoned() noexcept {
        if (!state_) return;
        // handler 抛出异常导致的析构：交给 trampoline 按异常类型失败。
        if (state_->dispatching.load() && std::uncaught_exceptions() > 0) return;
        if (AmCall* c = state_->take()) am_call_fail(c, "HANDLER_ERROR", "handler 未完成调用");
    }

    std::shared_ptr<detail::Pending<AmCall>> state_;
    std::string id_;
    std::string tool_name_;
    std::string arguments_json_;
    std::optional<std::string> idempotency_key_;
};

/// 一次资源读取。只能移动；必须完成一次，否则析构时以 HANDLER_ERROR 失败。
class Read {
public:
    explicit Read(std::shared_ptr<detail::Pending<AmRead>> state) : state_(std::move(state)) {
        if (AmRead* r = state_->raw.load()) resource_name_ = am_read_resource_name(r);
    }
    Read(Read&&) noexcept = default;
    Read& operator=(Read&& other) noexcept {
        if (this != &other) {
            finish_abandoned();
            state_ = std::move(other.state_);
            resource_name_ = std::move(other.resource_name_);
        }
        return *this;
    }
    Read(const Read&) = delete;
    Read& operator=(const Read&) = delete;
    ~Read() { finish_abandoned(); }

    const std::string& resource_name() const noexcept { return resource_name_; }
    bool pending() const noexcept { return state_ && state_->raw.load() != nullptr; }
    explicit operator bool() const noexcept { return pending(); }

    void complete(const std::string& contents_json) {
        AmRead* r = take();
        AmStatus s = am_read_complete(r, contents_json.c_str());
        if (s == AM_ERR_INVALID_JSON) state_->put_back(r);
        detail::check(s);
    }

    void fail(const std::string& kind, const std::string& message) {
        AmRead* r = take();
        detail::check(am_read_fail(r, kind.c_str(), message.c_str()));
    }

    /// 失败完成并附带结构化详情（JSON 文本，app_mcp.h v12）。非法 JSON 抛出 Error 且读取仍待完成。
    void fail(const std::string& kind, const std::string& message, const std::string& details_json) {
        AmRead* r = take();
        AmStatus s = am_read_fail_with_details(r, kind.c_str(), message.c_str(), details_json.c_str());
        if (s == AM_ERR_INVALID_JSON) state_->put_back(r);
        detail::check(s);
    }

    /// 以 USER_ACTION_REQUIRED 失败完成（app_mcp.h v12）；reason / uri 缺省时不出现在错误的 data 中。
    void fail_user_action(const std::string& message, const std::optional<std::string>& reason = std::nullopt,
                          const std::optional<std::string>& uri = std::nullopt) {
        AmRead* r = take();
        detail::check(am_read_fail_user_action(r, message.c_str(), detail::c_str_or_null(reason),
                                               detail::c_str_or_null(uri)));
    }

private:
    AmRead* take() {
        AmRead* r = state_ ? state_->take() : nullptr;
        if (!r) throw Error(AM_ERR_ALREADY_COMPLETED, "读取已完成");
        return r;
    }

    void finish_abandoned() noexcept {
        if (!state_) return;
        if (state_->dispatching.load() && std::uncaught_exceptions() > 0) return;
        if (AmRead* r = state_->take()) am_read_fail(r, "HANDLER_ERROR", "reader 未完成读取");
    }

    std::shared_ptr<detail::Pending<AmRead>> state_;
    std::string resource_name_;
};

/// 一次导航请求（Host 的 app/navigate，spec/protocol.md 3.4）。只能移动；必须 complete / fail / deny 一次，
/// 否则析构时以 NAVIGATION_FAILED 失败。完成函数可在任意线程调用。
class Navigate {
public:
    explicit Navigate(std::shared_ptr<detail::Pending<AmNavigate>> state) : state_(std::move(state)) {
        if (AmNavigate* n = state_->raw.load()) {
            page_ = am_navigate_page(n);
            if (const char* p = am_navigate_params_json(n)) params_json_ = p;
        }
    }
    Navigate(Navigate&&) noexcept = default;
    Navigate& operator=(Navigate&& other) noexcept {
        if (this != &other) {
            finish_abandoned();
            state_ = std::move(other.state_);
            page_ = std::move(other.page_);
            params_json_ = std::move(other.params_json_);
        }
        return *this;
    }
    Navigate(const Navigate&) = delete;
    Navigate& operator=(const Navigate&) = delete;
    ~Navigate() { finish_abandoned(); }

    /// 目标页面名。
    const std::string& page() const noexcept { return page_; }
    /// 页面参数 JSON 文本；Host 没有给出时为 nullopt。
    const std::optional<std::string>& params_json() const noexcept { return params_json_; }
    bool pending() const noexcept { return state_ && state_->raw.load() != nullptr; }
    explicit operator bool() const noexcept { return pending(); }

    /// 导航完成（最好在新页面的工具注册之后）。
    void complete() { detail::check(am_navigate_complete(take())); }
    /// 导航失败（NAVIGATION_FAILED）：页面不存在、参数不合法等。
    void fail(const std::string& message) { detail::check(am_navigate_fail(take(), message.c_str())); }
    /// 拒绝导航（NAVIGATION_DENIED）：如用户正在输入。message 面向模型 / 用户。
    void deny(const std::string& message) { detail::check(am_navigate_deny(take(), message.c_str())); }
    /// 以 USER_ACTION_REQUIRED 结束导航（v15）：如 App 在后台无法自行切到前台，发通知后以 reason "foreground"
    /// 与通知 / 深链接 uri 回复。reason 与 uri 缺省时不出现在错误的 data 中。
    void fail_user_action(const std::string& message, const std::optional<std::string>& reason = std::nullopt,
                          const std::optional<std::string>& uri = std::nullopt) {
        detail::check(am_navigate_fail_user_action(take(), message.c_str(), detail::c_str_or_null(reason),
                                                   detail::c_str_or_null(uri)));
    }

private:
    AmNavigate* take() {
        AmNavigate* n = state_ ? state_->take() : nullptr;
        if (!n) throw Error(AM_ERR_ALREADY_COMPLETED, "导航已完成");
        return n;
    }

    void finish_abandoned() noexcept {
        if (!state_) return;
        if (state_->dispatching.load() && std::uncaught_exceptions() > 0) return;
        if (AmNavigate* n = state_->take()) am_navigate_fail(n, "导航回调未完成导航");
    }

    std::shared_ptr<detail::Pending<AmNavigate>> state_;
    std::string page_;
    std::optional<std::string> params_json_;
};

using ToolHandler = std::function<void(Call)>;
using ResourceReader = std::function<void(Read)>;
using NavigationHandler = std::function<void(Navigate)>;

namespace detail {

/// 调用 / 读取共用的失败函数（C ABI 的 am_call_fail* 与 am_read_fail* 同签名、同语义）。
template <typename Raw>
struct FailOps {
    AmStatus (*fail)(Raw*, const char*, const char*);
    AmStatus (*fail_with_details)(Raw*, const char*, const char*, const char*);
    AmStatus (*fail_user_action)(Raw*, const char*, const char*, const char*);
};

/// 运行 handler / reader；抛出异常时按类型失败完成（未完成的）调用或读取。
/// @invariant UserActionRequired → *_fail_user_action；ToolCallError → *_fail_with_details（详情非法时退回不带详情）；
///            其他异常 → HANDLER_ERROR。
template <typename Raw, typename Invoke>
void run_with_failure(const std::shared_ptr<Pending<Raw>>& state, const FailOps<Raw>& ops, Invoke&& invoke) {
    try {
        invoke();
    } catch (const UserActionRequired& e) {
        if (Raw* r = state->take()) ops.fail_user_action(r, e.what(), c_str_or_null(e.reason()), c_str_or_null(e.uri()));
    } catch (const ToolCallError& e) {
        if (Raw* r = state->take()) {
            const char* details = e.details_json() ? e.details_json()->c_str() : nullptr;
            // 详情不是合法 JSON 时 call / read 不被消费：改为不带详情失败。
            if (ops.fail_with_details(r, e.kind().c_str(), e.what(), details) == AM_ERR_INVALID_JSON)
                ops.fail(r, e.kind().c_str(), e.what());
        }
    } catch (const std::exception& e) {
        if (Raw* r = state->take()) ops.fail(r, "HANDLER_ERROR", e.what());
    } catch (...) {
        if (Raw* r = state->take()) ops.fail(r, "HANDLER_ERROR", "未知异常");
    }
    state->dispatching.store(false);
}

inline void tool_trampoline(void* ud, AmCall* raw) {
    auto state = std::make_shared<Pending<AmCall>>(raw);
    static const FailOps<AmCall> ops{am_call_fail, am_call_fail_with_details, am_call_fail_user_action};
    run_with_failure(state, ops, [&] { (*static_cast<ToolHandler*>(ud))(Call(state)); });
}

inline void read_trampoline(void* ud, AmRead* raw) {
    auto state = std::make_shared<Pending<AmRead>>(raw);
    static const FailOps<AmRead> ops{am_read_fail, am_read_fail_with_details, am_read_fail_user_action};
    run_with_failure(state, ops, [&] { (*static_cast<ResourceReader*>(ud))(Read(state)); });
}

/// 运行导航回调；抛出 NavigationDenied → 拒绝，UserActionRequired → USER_ACTION_REQUIRED，其他异常 → 失败（未完成时）。
inline void navigate_trampoline(void* ud, AmNavigate* raw) {
    auto state = std::make_shared<Pending<AmNavigate>>(raw);
    try {
        (*static_cast<NavigationHandler*>(ud))(Navigate(state));
    } catch (const NavigationDenied& e) {
        if (AmNavigate* n = state->take()) am_navigate_deny(n, e.what());
    } catch (const UserActionRequired& e) {
        if (AmNavigate* n = state->take())
            am_navigate_fail_user_action(n, e.what(), c_str_or_null(e.reason()), c_str_or_null(e.uri()));
    } catch (const std::exception& e) {
        if (AmNavigate* n = state->take()) am_navigate_fail(n, e.what());
    } catch (...) {
        if (AmNavigate* n = state->take()) am_navigate_fail(n, "导航回调抛出了未知异常");
    }
    state->dispatching.store(false);
}

}  // namespace detail

}  // namespace app_mcp

#endif  // APP_MCP_HANDLERS_HPP
