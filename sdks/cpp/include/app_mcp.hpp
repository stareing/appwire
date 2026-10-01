// app_mcp.hpp —— app-mcp 的 C++17 header-only RAII 封装（基于 app_mcp.h）。
//
// 线程模型：handler、资源读取与客户端事件都在库的分发线程上调用，必须尽快返回。
// 需要在 UI 线程执行的工作，把 Call / Read 移动到 UI 线程后再完成（complete / fail 可在任意线程调用）。
// 例如 Qt：QMetaObject::invokeMethod(obj, [c = std::make_shared<Call>(std::move(call))] { ... });
//
// 所有权：
// - Client / Scope / Tool / Resource 析构时释放句柄，但不会注销工具或资源（注销用 dispose()）；
//   Client 析构会停止客户端。
// - Call / Read 为只能移动的对象，必须完成一次；析构时若尚未完成，以 HANDLER_ERROR 失败。
// - handler 抛出 ToolCallError 时按其 kind（及可选 details）失败，抛出其他 std::exception 时以 HANDLER_ERROR 失败。
//   需要用户本人操作（登录过期、权限未授予、需切到前台等）时抛出 UserActionRequired 或调用 Call::fail_user_action。
//
// 生命周期（spec/lifecycle.md，需要 AM_API_VERSION >= 3）：
// - ClientConfig::lifecycle 设置 persistent / idle / on-demand；空闲后与 Host 完成 app/sleep 并释放连接与运行时。
// - Client::handle_wake(args) 处理操作系统激活参数（命令行、URL、D-Bus action 参数），识别后回连。
// - Client::hold() / Call::hold() 返回 HoldGuard，析构时释放（RAII），期间不会自动休眠。
// - ClientCallbacks::on_idle_exit：residency 允许时，休眠完成后回调，App 自行决定是否退出。
// - 4e 功耗（spec/lifecycle.md 第 11、13 节）：ClientConfig::heartbeat、Lifecycle::host_absent_retries /
//   legacy_timers / merge_window_ms / sleep_on_background、ResourceOptions::realtime。
//   本封装不区分平台，默认 persistent（核心兼容）；平台默认（手机 on-demand、桌面 idle）由 App 自行设置。
//
// 工具声明与调用结果（spec/protocol.md 第 3 节、3.2；app_mcp.h v9）：
// - ToolOptions::annotations（标准 MCP 工具注解）、ToolOptions::output_schema_json（MCP outputSchema）；
//   ToolOptions::risk 为旧写法，优先用 annotations（同时声明时注解中的字段优先）。
// - Call::complete(const CallResult&)：业务状态（done / pending / partial / noop）、state_resource、summary、内容注解。
#ifndef APP_MCP_HPP
#define APP_MCP_HPP

#include <atomic>
#include <charconv>
#include <cstdint>
#include <exception>
#include <functional>
#include <memory>
#include <optional>
#include <stdexcept>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

#include "app_mcp.h"

#if !defined(AM_API_VERSION) || AM_API_VERSION < 3
#error "app_mcp.hpp 需要 app_mcp.h API 版本 3 或更高"
#endif

namespace app_mcp {

// ---------------------------------------------------------------------------
// 错误
// ---------------------------------------------------------------------------

/// 库函数返回非 AM_OK 时抛出。
class Error : public std::runtime_error {
public:
    Error(AmStatus status, const std::string& message) : std::runtime_error(message), status_(status) {}
    AmStatus status() const noexcept { return status_; }

private:
    AmStatus status_;
};

/// handler 中抛出，按指定的协议错误类别（如 "USER_REJECTED"）失败。
/// details_json（可选）为结构化详情 JSON：对象的字段合并进错误的 data，其他值放在 data.details。
class ToolCallError : public std::runtime_error {
public:
    ToolCallError(std::string kind, const std::string& message,
                  std::optional<std::string> details_json = std::nullopt)
        : std::runtime_error(message), kind_(std::move(kind)), details_json_(std::move(details_json)) {}
    const std::string& kind() const noexcept { return kind_; }
    const std::optional<std::string>& details_json() const noexcept { return details_json_; }

private:
    std::string kind_;
    std::optional<std::string> details_json_;
};

/// USER_ACTION_REQUIRED 的 data.reason 建议取值（spec/protocol.md 第 4 节；也可用其他字符串）。
namespace user_action_reason {
inline constexpr const char* login = "login";            ///< 登录已过期 / 未登录
inline constexpr const char* permission = "permission";  ///< 系统权限未授予
inline constexpr const char* foreground = "foreground";  ///< 需要把 App 切到前台
inline constexpr const char* confirm = "confirm";        ///< 需要用户在 App 内确认
}  // namespace user_action_reason

/// handler 中抛出，以 USER_ACTION_REQUIRED 失败（app_mcp.h v11）：需要用户本人操作后才能继续。
/// message 面向用户；reason（见 user_action_reason）与 uri（App 内入口，如深链接）可选，缺省时不出现在错误的 data 中。
class UserActionRequired : public ToolCallError {
public:
    explicit UserActionRequired(const std::string& message, std::optional<std::string> reason = std::nullopt,
                                std::optional<std::string> uri = std::nullopt)
        : ToolCallError("USER_ACTION_REQUIRED", message), reason_(std::move(reason)), uri_(std::move(uri)) {}
    const std::optional<std::string>& reason() const noexcept { return reason_; }
    const std::optional<std::string>& uri() const noexcept { return uri_; }

private:
    std::optional<std::string> reason_;
    std::optional<std::string> uri_;
};

namespace detail {

inline void check(AmStatus status) {
    if (status != AM_OK) {
        const char* msg = am_last_error_message();
        throw Error(status, msg ? msg : "");
    }
}

inline const char* c_str_or_null(const std::optional<std::string>& s) { return s ? s->c_str() : nullptr; }

/// 取走库分配的字符串并释放（即使复制时抛出异常也会释放）。
inline std::string take_string(char* s) {
    if (!s) return {};
    struct Free {
        char* p;
        ~Free() { am_string_free(p); }
    } guard{s};
    return std::string(s);
}

template <typename T>
void delete_fn(void* p) {
    delete static_cast<T*>(p);
}

/// Call / Read 的共享完成状态。原始指针只能被取走一次（原子交换），保证恰好完成一次。
template <typename Raw>
struct Pending {
    std::atomic<Raw*> raw;
    /// 分发 trampoline 正在执行时为 true：此时 handler 抛出的异常由 trampoline 负责失败。
    std::atomic<bool> dispatching{true};
    explicit Pending(Raw* r) : raw(r) {}
    Raw* take() { return raw.exchange(nullptr); }
    void put_back(Raw* r) { raw.store(r); }
};

}  // namespace detail

// ---------------------------------------------------------------------------
// 枚举与定义
// ---------------------------------------------------------------------------

using Status = AmStatus;
using Risk = AmRisk;
using Activation = AmActivation;
using Visibility = AmVisibility;
using CancelReason = AmCancelReason;
using StateStatus = AmStateStatus;
using LogLevel = AmLogLevel;
using ClientKind = AmClientKind;
using LifecycleMode = AmLifecycleMode;
using HeartbeatMode = AmHeartbeatMode;
using Residency = AmResidency;
using WakeKind = AmWakeKind;
using WakeReason = AmWakeReason;
using SleepReason = AmSleepReason;
/// 调用结果的业务状态（AM_RESULT_DONE / PENDING / PARTIAL / NOOP）。
using ResultStatus = AmResultStatus;

/// 本实例的唤醒描述（spec/lifecycle.md 第 5 节），随 app/sleep 上报。
struct WakeDescriptor {
    WakeKind kind = AM_WAKE_NONE;
    /// scheme / AUMID / bundle id / D-Bus 名 / 组件名 / URL。
    std::optional<std::string> target;
    /// 能否不把窗口带到前台就唤醒。
    bool background = false;
};

/// 生命周期策略（默认 persistent：不休眠）。
struct Lifecycle {
    LifecycleMode mode = AM_LIFECYCLE_PERSISTENT;
    uint64_t idle_timeout_ms = 60000;
    uint64_t hidden_idle_timeout_ms = 15000;
    uint64_t grace_ms = 10000;
    Residency residency = AM_RESIDENCY_KEEP;
    /// 不设置时 Host 回退到清单 launch。
    std::optional<WakeDescriptor> wake;
    /// idle / on-demand 下连续多少次"Host 不在"后转休眠（第 11 节 A2）；0 = 一直重连。
    uint32_t host_absent_retries = 3;
    /// true：回退到 4e 之前的定时器行为（第 11、13 节）。
    bool legacy_timers = false;
    /// 调用 / 资源读取后的合并窗口（第 13 节 B1）；0 = 不留窗口（调用后只看租约）。
    uint64_t merge_window_ms = 2000;
    /// true：idle / on-demand 下进入后台且空闲时立即休眠，不等租约（第 13 节 B4）。
    bool sleep_on_background = false;
};

struct StateInfo {
    StateStatus status = AM_STATE_IDLE;
    uint64_t retry_in_ms = 0;
    /// REJECTED / HOST_MISMATCH（对端不是 app-mcp 或属于其他用户）时非空；BACKOFF 在连接失败等情况下也非空。
    std::string reason;
    /// 与 reason 对应的错误码（spec/protocol.md 10.1，如 "HOST_NOT_RUNNING"）；没有时为 nullopt。
    std::optional<std::string> code;
};

/// 标准 MCP 工具注解，原样转发给 Agent（本库不据此做判断）；未设置的字段不声明。
struct ToolAnnotations {
    std::optional<std::string> title;
    /// 不修改任何状态。
    std::optional<bool> read_only_hint;
    /// 可能做出破坏性 / 不可撤销的修改（只在非只读时有意义）。
    std::optional<bool> destructive_hint;
    /// 以相同参数重复调用没有额外效果（只在非只读时有意义）。
    std::optional<bool> idempotent_hint;
    /// 会与外部世界交互（网络、第三方、其他用户可见）。
    std::optional<bool> open_world_hint;
};

struct ToolOptions {
    /// JSON Schema 文本（type 必须为 object）；为空表示无参数。
    std::optional<std::string> input_schema_json;
    /// 旧写法：优先用 annotations。两者同时声明时注解中的字段优先，缺少的按 risk 推导。
    Risk risk = AM_RISK_WRITE;
    Activation activation = AM_ACTIVATION_NONE;
    std::optional<std::string> title;
    bool enabled = true;
    /// 标准 MCP 工具注解；为空表示不声明（Host 按 risk 推导）。
    std::optional<ToolAnnotations> annotations;
    /// 结果的 JSON Schema 文本（MCP outputSchema）；为空表示不声明。
    std::optional<std::string> output_schema_json;
};

/// 内容面向谁（MCP 内容注解 audience）。
enum class Audience { User, Assistant };

/// 结果内容的标注（MCP 内容注解），Host 原样转发；未设置的字段不声明。
struct ContentAnnotations {
    std::optional<std::vector<Audience>> audience;
    /// 重要程度，0（可选）到 1（必需）。
    std::optional<double> priority;
    /// 最后修改时刻（ISO 8601）。
    std::optional<std::string> last_modified;
};

/// 调用成功的完整结果（Call::complete(const CallResult&)）。默认值 = 无返回值（null）、done。
struct CallResult {
    /// 返回值 JSON 文本；为空表示无返回值（null，Host 对模型输出"已完成"）。
    std::optional<std::string> data_json;
    /// 调用后内容可能已变化的资源名。
    std::vector<std::string> state_hints;
    ResultStatus status = AM_RESULT_DONE;
    /// PENDING 时可读取后续状态的资源名。
    std::optional<std::string> state_resource;
    /// 一句面向模型 / 用户的结论（PARTIAL 时说明完成了哪部分）。
    std::optional<std::string> summary;
    std::optional<ContentAnnotations> annotations;
};

struct ResourceOptions {
    /// 为空时 application/json。
    std::optional<std::string> mime_type;
    /// 需实时推送（spec/lifecycle.md 第 13 节 B3）：被订阅时保持连接、休眠中变化时回连推送。
    bool realtime = false;
};

/// App 总览（Host 在模型首次接触该 App 时附带）。
struct AppOverview {
    std::string summary;               // 一句话简介（≤ 100 字符）
    std::optional<std::string> body;   // Markdown 正文（≤ 2000 字符）
    std::optional<std::string> locale; // 如 "zh-CN"
};

struct ClientConfig {
    std::string app_id;
    std::string app_name;
    std::optional<std::string> instance_id;
    std::optional<std::string> host_url;  // 默认：APP_MCP_ENDPOINT → 登记文件 ~/.app-mcp/run/endpoints.json → 平台默认本地 IPC 端点（unix: / pipe:）→ ws://127.0.0.1:7717/app
    std::optional<std::string> app_version;
    std::optional<std::string> instance_title;
    std::optional<std::string> token;
    std::optional<std::string> launch_token;
    ClientKind client_kind = AM_CLIENT_NATIVE;
    uint32_t max_concurrent_calls = 1;
    std::optional<AppOverview> overview;
    Lifecycle lifecycle;
    /// 建立连接的超时；0 表示默认 5000ms。
    uint32_t connect_timeout_ms = 0;
    /// 心跳策略（spec/lifecycle.md 第 11 节 A3）；AUTO：本地 IPC / 桌面本机回环不发。
    HeartbeatMode heartbeat = AM_HEARTBEAT_AUTO;
};

struct ClientCallbacks {
    std::function<void(const StateInfo&)> on_state;
    std::function<void(const std::string& token)> on_paired;
    std::function<void(LogLevel, const std::string&)> on_log;
    /// 已休眠且 residency 允许退出（在分发线程上调用）。不要在回调里直接析构 Client：
    /// 通知主线程 / 事件循环退出即可。
    std::function<void()> on_idle_exit;
};

/// 把字符串编码为 JSON 字符串字面量（含引号），便于手写 JSON 结果。
inline std::string json_quote(std::string_view s) {
    static const char* hex = "0123456789abcdef";
    std::string out;
    out.reserve(s.size() + 2);
    out.push_back('"');
    for (unsigned char ch : s) {
        switch (ch) {
            case '"': out += "\\\""; break;
            case '\\': out += "\\\\"; break;
            case '\n': out += "\\n"; break;
            case '\r': out += "\\r"; break;
            case '\t': out += "\\t"; break;
            default:
                if (ch < 0x20) {
                    out += "\\u00";
                    out.push_back(hex[ch >> 4]);
                    out.push_back(hex[ch & 0xf]);
                } else {
                    out.push_back(static_cast<char>(ch));
                }
        }
    }
    out.push_back('"');
    return out;
}

namespace detail {

/// @compat C ABI 的 host_absent_retries：0 = 默认 3、负数 = 一直重连；封装层 0 = 一直重连。
inline int32_t encode_host_absent_retries(uint32_t retries) noexcept {
    if (retries == 0) return -1;
    return retries > static_cast<uint32_t>(INT32_MAX) ? INT32_MAX : static_cast<int32_t>(retries);
}

/// @compat C ABI 的 merge_window_ms：0 = 默认 2000、负数 = 不留窗口；封装层 0 = 不留窗口。
inline int64_t encode_merge_window_ms(uint64_t ms) noexcept {
    if (ms == 0) return -1;
    return ms > static_cast<uint64_t>(INT64_MAX) ? INT64_MAX : static_cast<int64_t>(ms);
}

/// ClientConfig → AmLifecycle + AmClientOptions（不含回调）。
/// @invariant opts->lifecycle 指向 *lc，lc.wake_target 借用 config 的字符串；二者都不能比 config 活得久。
inline void fill_client_options(const ClientConfig& config, AmLifecycle* lc, AmClientOptions* opts) {
    am_lifecycle_init(lc);
    lc->mode = config.lifecycle.mode;
    lc->idle_timeout_ms = config.lifecycle.idle_timeout_ms;
    lc->hidden_idle_timeout_ms = config.lifecycle.hidden_idle_timeout_ms;
    lc->grace_ms = config.lifecycle.grace_ms;
    lc->residency = config.lifecycle.residency;
    if (config.lifecycle.wake) {
        lc->wake_kind = config.lifecycle.wake->kind;
        lc->wake_target = c_str_or_null(config.lifecycle.wake->target);
        lc->wake_background = config.lifecycle.wake->background;
    }
    *opts = AmClientOptions{};
    opts->struct_size = sizeof(AmClientOptions);
    opts->lifecycle = lc;
    opts->connect_timeout_ms = config.connect_timeout_ms;
    opts->heartbeat = config.heartbeat;
    opts->host_absent_retries = encode_host_absent_retries(config.lifecycle.host_absent_retries);
    opts->legacy_timers = config.lifecycle.legacy_timers;
    opts->merge_window_ms = encode_merge_window_ms(config.lifecycle.merge_window_ms);
    opts->sleep_on_background = config.lifecycle.sleep_on_background;
}

/// 逐个追加 JSON 对象成员（跳过未设置的可选值）。
class JsonObject {
public:
    void field(const char* key, const std::optional<std::string>& v) {
        if (v) key_(key) += json_quote(*v);
    }
    void field(const char* key, const std::optional<bool>& v) {
        if (v) key_(key) += *v ? "true" : "false";
    }
    /// @invariant 非有限值（NaN / Inf）不是合法 JSON，不输出。
    void field(const char* key, const std::optional<double>& v) {
        if (!v || !(*v == *v) || *v > 1.7976931348623157e308 || *v < -1.7976931348623157e308) return;
        char buf[32];
        auto r = std::to_chars(buf, buf + sizeof buf, *v);
        key_(key).append(buf, r.ptr);
    }
    void raw(const char* key, const std::string& json) { key_(key) += json; }
    std::string finish() { return text_ + "}"; }

private:
    std::string& key_(const char* key) {
        text_ += text_.size() > 1 ? "," : "";
        text_ += json_quote(key);
        text_ += ":";
        return text_;
    }
    std::string text_ = "{";
};

inline std::string to_json(const ToolAnnotations& a) {
    JsonObject o;
    o.field("title", a.title);
    o.field("readOnlyHint", a.read_only_hint);
    o.field("destructiveHint", a.destructive_hint);
    o.field("idempotentHint", a.idempotent_hint);
    o.field("openWorldHint", a.open_world_hint);
    return o.finish();
}

inline std::string to_json(const ContentAnnotations& a) {
    JsonObject o;
    if (a.audience) {
        std::string list = "[";
        for (Audience who : *a.audience) {
            if (list.size() > 1) list += ",";
            list += who == Audience::User ? "\"user\"" : "\"assistant\"";
        }
        o.raw("audience", list + "]");
    }
    o.field("priority", a.priority);
    o.field("lastModified", a.last_modified);
    return o.finish();
}

/// ToolOptions → AmToolOptions。
/// @invariant 指针借用 annotations_json（由调用方保持存活）与 options 的字符串。
inline AmToolOptions tool_options(const ToolOptions& options, const std::optional<std::string>& annotations_json) {
    AmToolOptions o{};
    o.struct_size = sizeof(AmToolOptions);
    o.annotations_json = c_str_or_null(annotations_json);
    o.output_schema_json = c_str_or_null(options.output_schema_json);
    return o;
}

inline std::optional<std::string> annotations_json(const ToolOptions& options) {
    if (!options.annotations) return std::nullopt;
    return to_json(*options.annotations);
}

/// 客户端回调的 user_data：用户回调 + 客户端句柄（状态回调里查询错误码用）。
struct ClientCallbackHolder {
    ClientCallbacks callbacks;
    /// am_client_new_ex 成功后写入；句柄在 am_client_free 前一直有效（释放时先停掉回调线程）。
    std::atomic<AmClient*> client{nullptr};
};

/// 当前状态的错误码（am_client_state_code）。
inline std::optional<std::string> state_code(const AmClient* client) {
    char* code = nullptr;
    check(am_client_state_code(client, &code));
    if (!code) return std::nullopt;
    return take_string(code);
}

/// 只有 BACKOFF / REJECTED / HOST_MISMATCH 带错误码（spec/protocol.md 10.1）。
constexpr bool status_has_code(AmStateStatus status) {
    return status == AM_STATE_BACKOFF || status == AM_STATE_REJECTED || status == AM_STATE_HOST_MISMATCH;
}

}  // namespace detail

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

using ToolHandler = std::function<void(Call)>;
using ResourceReader = std::function<void(Read)>;

namespace detail {

inline void tool_trampoline(void* ud, AmCall* raw) {
    auto state = std::make_shared<Pending<AmCall>>(raw);
    try {
        (*static_cast<ToolHandler*>(ud))(Call(state));
    } catch (const UserActionRequired& e) {
        if (AmCall* c = state->take())
            am_call_fail_user_action(c, e.what(), c_str_or_null(e.reason()), c_str_or_null(e.uri()));
    } catch (const ToolCallError& e) {
        if (AmCall* c = state->take()) {
            const char* details = e.details_json() ? e.details_json()->c_str() : nullptr;
            // 详情不是合法 JSON 时 call 不被消费：改为不带详情失败。
            if (am_call_fail_with_details(c, e.kind().c_str(), e.what(), details) == AM_ERR_INVALID_JSON)
                am_call_fail(c, e.kind().c_str(), e.what());
        }
    } catch (const std::exception& e) {
        if (AmCall* c = state->take()) am_call_fail(c, "HANDLER_ERROR", e.what());
    } catch (...) {
        if (AmCall* c = state->take()) am_call_fail(c, "HANDLER_ERROR", "未知异常");
    }
    state->dispatching.store(false);
}

inline void read_trampoline(void* ud, AmRead* raw) {
    auto state = std::make_shared<Pending<AmRead>>(raw);
    try {
        (*static_cast<ResourceReader*>(ud))(Read(state));
    } catch (const ToolCallError& e) {
        if (AmRead* r = state->take()) am_read_fail(r, e.kind().c_str(), e.what());
    } catch (const std::exception& e) {
        if (AmRead* r = state->take()) am_read_fail(r, "HANDLER_ERROR", e.what());
    } catch (...) {
        if (AmRead* r = state->take()) am_read_fail(r, "HANDLER_ERROR", "未知异常");
    }
    state->dispatching.store(false);
}

}  // namespace detail

// ---------------------------------------------------------------------------
// Tool / Resource / Scope
// ---------------------------------------------------------------------------

/// 已注册的工具。析构只释放句柄，不注销（注销用 dispose()）。
class Tool {
public:
    Tool() = default;
    explicit Tool(AmTool* h) : h_(h) {}
    Tool(Tool&& o) noexcept : h_(std::exchange(o.h_, nullptr)) {}
    Tool& operator=(Tool&& o) noexcept {
        if (this != &o) {
            am_tool_free(h_);
            h_ = std::exchange(o.h_, nullptr);
        }
        return *this;
    }
    Tool(const Tool&) = delete;
    Tool& operator=(const Tool&) = delete;
    ~Tool() { am_tool_free(h_); }

    explicit operator bool() const noexcept { return h_ != nullptr; }
    AmTool* get() const noexcept { return h_; }

    /// 用新定义整体替换（名称不变；options 中未设置的 annotations / output_schema_json 表示清除该声明）。
    void update(const std::string& description, const ToolOptions& options = {}) {
        AmToolSpec spec{};
        spec.name = nullptr;
        spec.description = description.c_str();
        spec.input_schema_json = detail::c_str_or_null(options.input_schema_json);
        spec.risk = options.risk;
        spec.activation = options.activation;
        spec.title = detail::c_str_or_null(options.title);
        spec.enabled = options.enabled;
        auto annotations = detail::annotations_json(options);
        AmToolOptions topts = detail::tool_options(options, annotations);
        detail::check(am_tool_update_ex(h_, &spec, &topts));
    }
    void set_enabled(bool enabled) { detail::check(am_tool_set_enabled(h_, enabled)); }
    void dispose() { detail::check(am_tool_dispose(h_)); }

private:
    AmTool* h_ = nullptr;
};

class Resource {
public:
    Resource() = default;
    explicit Resource(AmResource* h) : h_(h) {}
    Resource(Resource&& o) noexcept : h_(std::exchange(o.h_, nullptr)) {}
    Resource& operator=(Resource&& o) noexcept {
        if (this != &o) {
            am_resource_free(h_);
            h_ = std::exchange(o.h_, nullptr);
        }
        return *this;
    }
    Resource(const Resource&) = delete;
    Resource& operator=(const Resource&) = delete;
    ~Resource() { am_resource_free(h_); }

    explicit operator bool() const noexcept { return h_ != nullptr; }
    AmResource* get() const noexcept { return h_; }

    void notify_changed() { detail::check(am_resource_notify_changed(h_)); }
    void dispose() { detail::check(am_resource_dispose(h_)); }

private:
    AmResource* h_ = nullptr;
};

/// 作用域：dispose() 注销其下全部工具、资源与子作用域。
class Scope {
public:
    Scope() = default;
    explicit Scope(AmScope* h) : h_(h) {}
    Scope(Scope&& o) noexcept : h_(std::exchange(o.h_, nullptr)) {}
    Scope& operator=(Scope&& o) noexcept {
        if (this != &o) {
            am_scope_free(h_);
            h_ = std::exchange(o.h_, nullptr);
        }
        return *this;
    }
    Scope(const Scope&) = delete;
    Scope& operator=(const Scope&) = delete;
    ~Scope() { am_scope_free(h_); }

    explicit operator bool() const noexcept { return h_ != nullptr; }
    AmScope* get() const noexcept { return h_; }

    Scope create_scope(const std::string& name) {
        AmScope* out = nullptr;
        detail::check(am_scope_create(h_, name.c_str(), &out));
        return Scope(out);
    }

    Tool register_tool(const std::string& name, const std::string& description, ToolHandler handler,
                       const ToolOptions& options = {}) {
        AmToolSpec spec{};
        spec.name = name.c_str();
        spec.description = description.c_str();
        spec.input_schema_json = detail::c_str_or_null(options.input_schema_json);
        spec.risk = options.risk;
        spec.activation = options.activation;
        spec.title = detail::c_str_or_null(options.title);
        spec.enabled = options.enabled;
        auto annotations = detail::annotations_json(options);
        AmToolOptions topts = detail::tool_options(options, annotations);
        // 所有权交给库：无论成功与否，库都会调用 delete_fn 释放。
        auto* holder = new ToolHandler(std::move(handler));
        AmTool* out = nullptr;
        detail::check(am_tool_register_ex(h_, &spec, &topts, &detail::tool_trampoline, holder,
                                          &detail::delete_fn<ToolHandler>, &out));
        return Tool(out);
    }

    Resource register_resource(const std::string& name, const std::string& description, ResourceReader reader,
                               const std::optional<std::string>& mime_type = std::nullopt) {
        return register_resource(name, description, std::move(reader), ResourceOptions{mime_type, false});
    }

    Resource register_resource(const std::string& name, const std::string& description, ResourceReader reader,
                               const ResourceOptions& options) {
        AmResourceSpec spec{};
        spec.name = name.c_str();
        spec.description = description.c_str();
        spec.mime_type = detail::c_str_or_null(options.mime_type);
        AmResourceOptions ropts{};
        ropts.struct_size = sizeof(AmResourceOptions);
        ropts.realtime = options.realtime;
        auto* holder = new ResourceReader(std::move(reader));
        AmResource* out = nullptr;
        detail::check(am_resource_register_ex(h_, &spec, &ropts, &detail::read_trampoline, holder,
                                              &detail::delete_fn<ResourceReader>, &out));
        return Resource(out);
    }

    void dispose() { detail::check(am_scope_dispose(h_)); }

private:
    AmScope* h_ = nullptr;
};

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

class Client {
public:
    explicit Client(const ClientConfig& config, ClientCallbacks callbacks = {}) {
        AmClientConfig c{};
        c.app_id = config.app_id.c_str();
        c.app_name = config.app_name.c_str();
        c.instance_id = detail::c_str_or_null(config.instance_id);
        c.host_url = detail::c_str_or_null(config.host_url);
        c.app_version = detail::c_str_or_null(config.app_version);
        c.instance_title = detail::c_str_or_null(config.instance_title);
        c.token = detail::c_str_or_null(config.token);
        c.launch_token = detail::c_str_or_null(config.launch_token);
        c.client_kind = config.client_kind;
        c.max_concurrent_calls = config.max_concurrent_calls;
        if (config.overview) {
            c.overview_summary = config.overview->summary.c_str();
            c.overview_body = detail::c_str_or_null(config.overview->body);
            c.overview_locale = detail::c_str_or_null(config.overview->locale);
        }

        AmLifecycle lc{};
        AmClientOptions opts{};
        detail::fill_client_options(config, &lc, &opts);
        if (callbacks.on_idle_exit) {
            opts.on_idle_exit = [](void* ud) {
                auto& h = static_cast<detail::ClientCallbackHolder*>(ud)->callbacks;
                try {
                    if (h.on_idle_exit) h.on_idle_exit();
                } catch (...) {
                }
            };
        }

        auto* holder = new detail::ClientCallbackHolder{std::move(callbacks), {}};
        AmClientCallbacks cb{};
        // 回调中的字符串归回调方所有（AM_API_VERSION 2），先取走（释放）再调用用户函数。
        // @why 状态回调签名没有 code（C ABI v6 只新增查询函数），回调时用 am_client_state_code 取当前状态的码；
        //      回调异步分发，状态可能已再次变化，此时 code 反映的是更新后的状态（可能为 nullopt）。
        cb.on_state = [](void* ud, AmStateStatus status, uint64_t retry, char* reason) {
            auto* h = static_cast<detail::ClientCallbackHolder*>(ud);
            try {
                StateInfo info{status, retry, detail::take_string(reason), std::nullopt};
                AmClient* client = h->client.load();
                if (client && detail::status_has_code(status)) info.code = detail::state_code(client);
                if (h->callbacks.on_state) h->callbacks.on_state(info);
            } catch (...) {
            }
        };
        cb.on_paired = [](void* ud, char* token) {
            auto& h = static_cast<detail::ClientCallbackHolder*>(ud)->callbacks;
            try {
                std::string t = detail::take_string(token);
                if (h.on_paired) h.on_paired(t);
            } catch (...) {
            }
        };
        cb.on_log = [](void* ud, AmLogLevel level, char* message) {
            auto& h = static_cast<detail::ClientCallbackHolder*>(ud)->callbacks;
            try {
                std::string m = detail::take_string(message);
                if (h.on_log) h.on_log(level, m);
            } catch (...) {
            }
        };
        cb.user_data = holder;
        cb.free_user_data = &detail::delete_fn<detail::ClientCallbackHolder>;
        // 失败时库已调用 free_user_data 释放 holder，不能再访问。
        detail::check(am_client_new_ex(&c, &cb, &opts, &h_));
        holder->client.store(h_);
    }
    Client(Client&& o) noexcept : h_(std::exchange(o.h_, nullptr)) {}
    Client& operator=(Client&& o) noexcept {
        if (this != &o) {
            am_client_free(h_);
            h_ = std::exchange(o.h_, nullptr);
        }
        return *this;
    }
    Client(const Client&) = delete;
    Client& operator=(const Client&) = delete;
    /// 停止并释放客户端（阻塞到后台线程结束）。不要在回调线程上析构。
    ~Client() { am_client_free(h_); }

    AmClient* get() const noexcept { return h_; }

    void start() { detail::check(am_client_start(h_)); }
    void stop() { detail::check(am_client_stop(h_)); }
    void set_visibility(Visibility visibility, bool focused) {
        detail::check(am_client_set_visibility(h_, visibility, focused));
    }

    StateInfo state() const {
        StateInfo info;
        char* reason = nullptr;
        detail::check(am_client_state(h_, &info.status, &info.retry_in_ms, &reason));
        info.reason = detail::take_string(reason);
        info.code = detail::state_code(h_);
        return info;
    }

    std::string instance_id() const { return detail::take_string(am_client_instance_id(h_)); }

    std::optional<std::string> token() const {
        char* t = am_client_token(h_);
        if (!t) return std::nullopt;
        return detail::take_string(t);
    }

    /// Host 为当前连接分配的连接 ID（spec/protocol.md 10.3），与 Host 日志中的 cid 对应；未连接时为 nullopt。
    std::optional<std::string> connection_id() const {
        char* id = nullptr;
        detail::check(am_client_connection_id(h_, &id));
        if (!id) return std::nullopt;
        return detail::take_string(id);
    }

    // ---- 生命周期（spec/lifecycle.md） ----

    /// 处理操作系统激活参数 / URL；不是本 SDK 的唤醒返回 false。可在 start() 之前调用（冷启动唤醒）。
    bool handle_wake(const std::string& args) { return am_client_handle_wake(h_, args.c_str()); }
    /// 逐个尝试命令行参数（如 main 的 argv），任一被识别即返回 true。
    bool handle_wake(int argc, const char* const* argv) {
        for (int i = 0; i < argc; ++i) {
            if (argv[i] && am_client_handle_wake(h_, argv[i])) return true;
        }
        return false;
    }
    /// App 主动回连。返回是否因此发起了回连。
    bool wake(WakeReason reason = AM_WAKE_REASON_APP) {
        bool started = false;
        detail::check(am_client_wake_with_reason(h_, reason, &started));
        return started;
    }
    /// on-demand 模式下主动连接；尚未 start 时等同于 start。
    bool connect_now() {
        bool started = false;
        detail::check(am_client_connect_now(h_, &started));
        return started;
    }
    /// App 主动请求休眠。返回是否有效果。
    bool sleep(SleepReason reason = AM_SLEEP_REASON_APP) {
        bool changed = false;
        detail::check(am_client_sleep_with_reason(h_, reason, &changed));
        return changed;
    }
    /// 临时阻止自动休眠，直到返回的 HoldGuard 释放。
    HoldGuard hold() {
        AmHold* h = nullptr;
        detail::check(am_client_hold(h_, &h));
        return HoldGuard(h);
    }
    /// 当前工具与资源定义的摘要（16 个十六进制字符）。
    std::string tools_hash() const {
        char* s = am_client_tools_hash(h_);
        if (!s) throw Error(h_ ? AM_ERR_STOPPED : AM_ERR_INVALID_ARGUMENT, am_last_error_message());
        return detail::take_string(s);
    }

    /// 根作用域：dispose() 注销全部工具与资源。
    Scope root_scope() {
        AmScope* out = nullptr;
        detail::check(am_client_root_scope(h_, &out));
        return Scope(out);
    }

    Scope create_scope(const std::string& name) { return root_scope().create_scope(name); }

    Tool register_tool(const std::string& name, const std::string& description, ToolHandler handler,
                       const ToolOptions& options = {}) {
        return root_scope().register_tool(name, description, std::move(handler), options);
    }

    Resource register_resource(const std::string& name, const std::string& description, ResourceReader reader,
                               const std::optional<std::string>& mime_type = std::nullopt) {
        return root_scope().register_resource(name, description, std::move(reader), mime_type);
    }

    Resource register_resource(const std::string& name, const std::string& description, ResourceReader reader,
                               const ResourceOptions& options) {
        return root_scope().register_resource(name, description, std::move(reader), options);
    }

private:
    AmClient* h_ = nullptr;
};

inline std::string version() { return am_version(); }

/// 从激活参数中提取唤醒令牌（不需要客户端；如单实例转发前判断参数是否为唤醒）。
inline std::optional<std::string> parse_wake_token(const std::string& args) {
    char* t = am_parse_wake_token(args.c_str());
    if (!t) return std::nullopt;
    return detail::take_string(t);
}

}  // namespace app_mcp

#endif  // APP_MCP_HPP
