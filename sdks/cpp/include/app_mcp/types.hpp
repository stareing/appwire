// app_mcp/types.hpp —— 枚举、定义、配置与 JSON 编码（由 app_mcp.hpp 包含；直接包含 app_mcp.hpp 即可）。
#ifndef APP_MCP_TYPES_HPP
#define APP_MCP_TYPES_HPP

#include "errors.hpp"

namespace app_mcp {

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
/// 用户正在操作（Client::set_busy）期间写调用的处理方式（AM_BUSY_REJECT / AM_BUSY_QUEUE；spec/protocol.md 5.3）。
using BusyPolicy = AmBusyPolicy;

/// 工具对界面的依赖（spec/protocol.md 3.4）。
enum class Surface {
    /// 不依赖界面：后台可调、可唤醒（缺省）。
    App = AM_SURFACE_APP,
    /// 依赖界面：只在所在界面可见且处于最上层时注册。
    View = AM_SURFACE_VIEW,
};

/// 结果缓存的范围（spec/protocol.md 3.6，app_mcp.h v22）。
enum class CacheScope {
    /// 按调用方隔离（缺省）。
    Private = AM_CACHE_PRIVATE,
    /// 全体调用方共用：只用于与调用方无关的数据。
    Shared = AM_CACHE_SHARED,
};

/// 结果缓存声明（spec/protocol.md 3.6，app_mcp.h v22）：Hub 在 ttl_ms 内复用相同请求的结果（命中不唤醒 App）。
/// ttl_ms 须在 1..=86400000 之间，否则注册 / 更新抛出 InvalidConfig。
struct CachePolicy {
    uint64_t ttl_ms = 0;
    CacheScope scope = CacheScope::Private;
};

/// 工具弃用声明（spec/protocol.md 3.7，app_mcp.h v23）：照常列出与调用，Hub 把声明原样交给 Agent。
/// 格式不合法（message 为空或超过 500 个字符、replacement 不是局部名或指向自身、until 不是 YYYY-MM-DD）时
/// 注册 / 更新抛出 InvalidConfig。
struct Deprecation {
    /// 为什么弃用、该怎么做（面向模型）。
    std::string message;
    /// 替代工具在同一 App 中的局部名；为空表示不给出。
    std::optional<std::string> replacement;
    /// 计划移除日期（YYYY-MM-DD），只作提示；为空表示不给出。
    std::optional<std::string> until;
};

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
    /// 对界面的依赖（app_mcp.h v14）。
    Surface surface = Surface::App;
    /// 所在页面名 [a-zA-Z0-9_.-]{1,64}；为空表示不声明。Hub 在该工具未注册时据此导航（v14）。
    std::optional<std::string> page;
    /// 只对 Surface::View 有意义：App 在后台、本工具不可调用时 Hub 改调的同 App app 工具本地名；为空表示不声明（v15）。
    std::optional<std::string> background_tool;
    /// 本工具同时执行的调用上限；0 = 不单独限制，只受 ClientConfig::max_concurrent_calls 约束（v18，spec/protocol.md 5.3）。
    /// 只在 SDK 内调度，不同步给 Host。
    uint32_t concurrency = 0;
    /// 互斥组名 [a-zA-Z0-9_.-]{1,64}：同组的工具同一时刻至多一个在执行；为空表示不互斥（v18）。
    std::optional<std::string> exclusive;
    /// 实现的标准意图（spec/intents.md），每项 "<动词>@<主版本>"（如 "message.send@1"），最多 4 项、不重复；
    /// 为空表示不声明（v21）。格式不合法时注册 / 更新抛出 InvalidName。
    std::vector<std::string> implements;
    /// 结果缓存声明（v22）；只对生效注解只读的工具生效（否则照常注册并记警告日志）。为空表示不声明（更新时清除）。
    std::optional<CachePolicy> cache;
    /// 弃用声明（v23）；为空表示不声明（更新时清除）。
    std::optional<Deprecation> deprecated;
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
    /// 资源内容的标注（MCP 内容注解，app_mcp.h v13），Hub 放到 MCP resources/list 的资源注解上；为空表示不声明。
    std::optional<ContentAnnotations> annotations;
    /// 读取结果缓存声明（v22）；为空表示不声明。
    std::optional<CachePolicy> cache;
};

/// 调用去重（spec/protocol.md 3.3，app_mcp.h v13）：同一 callId 在有效期内重复到达时重放首次结果，不再执行 handler。
/// 任一字段为 0 关闭去重。
struct CallDedup {
    /// 首次结果的保留时长。
    uint64_t ttl_ms = 300000;
    /// 最多保留的结果数，超出淘汰最早的。
    uint32_t max_entries = 64;
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
    /// 排队中（等并发名额 / 互斥组）的调用上限；超出时新调用以 RATE_LIMITED（details scope "queue"）拒绝。
    /// 0 = 不限（spec/protocol.md 5.3，app_mcp.h v18）。
    uint32_t max_queued_calls = 64;
    /// 用户正在操作（Client::set_busy(true)）期间写调用的处理方式：AM_BUSY_REJECT（缺省）以 RATE_LIMITED
    /// （details scope "busy"）拒绝；AM_BUSY_QUEUE 排队到 set_busy(false) 后按序执行（app_mcp.h v19）。
    /// 运行时可用 Client::set_busy_policy 修改。
    BusyPolicy busy_policy = AM_BUSY_REJECT;
    std::optional<AppOverview> overview;
    Lifecycle lifecycle;
    /// 建立连接的超时；0 表示默认 5000ms。
    uint32_t connect_timeout_ms = 0;
    /// 心跳策略（spec/lifecycle.md 第 11 节 A3）；AUTO：本地 IPC / 桌面本机回环不发。
    HeartbeatMode heartbeat = AM_HEARTBEAT_AUTO;
    /// 调用去重（默认保留 5 分钟、最多 64 条；任一字段为 0 关闭）。
    CallDedup call_dedup;
    /// 按名寻址（spec/naming.md，app_mcp.h v17）：start() 后在系统名字服务登记，由 Hub 按名拨入
    /// （Linux：D-Bus dev.appmcp.App.<app_id>；Windows：命名管道 \\.\pipe\appmcp-<用户 SID>-<app_id>）。
    /// 需 `app-mcp-host app install --app-id <app_id> --exec <本程序>` 登记；通常与 AM_LIFECYCLE_ON_DEMAND +
    /// AM_RESIDENCY_EXIT_WHEN_IDLE 同用。本平台不支持时经 on_log 报告，其余照常。
    bool register_name = false;
    /// 登记实例名（[a-z][a-z0-9-]{0,31}，不能是 "default"），另登记 dev.appmcp.App.<app_id>.<实例>；
    /// 不合法时 Client 构造抛出 Error（AM_ERR_INVALID_CONFIG）。
    std::optional<std::string> name_instance;
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

/// @compat C ABI 的 call_dedup_*：0 = 默认、负数 = 关闭；封装层 0 = 关闭。
inline int64_t encode_dedup_ttl_ms(uint64_t ms) noexcept {
    if (ms == 0) return -1;
    return ms > static_cast<uint64_t>(INT64_MAX) ? INT64_MAX : static_cast<int64_t>(ms);
}
inline int32_t encode_dedup_max_entries(uint32_t n) noexcept {
    if (n == 0) return -1;
    return n > static_cast<uint32_t>(INT32_MAX) ? INT32_MAX : static_cast<int32_t>(n);
}
/// @compat C ABI 的 max_queued_calls：0 = 默认 64、负数 = 不限；封装层 0 = 不限。
inline int32_t encode_max_queued_calls(uint32_t n) noexcept { return encode_dedup_max_entries(n); }

/// ClientConfig → AmLifecycle + AmClientOptions（不含回调）。
/// @invariant opts->lifecycle 指向 *lc，lc.wake_target 与 opts->name_instance 借用 config 的字符串；二者都不能比 config 活得久。
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
    opts->call_dedup_ttl_ms = encode_dedup_ttl_ms(config.call_dedup.ttl_ms);
    opts->call_dedup_max_entries = encode_dedup_max_entries(config.call_dedup.max_entries);
    opts->register_name = config.register_name;
    opts->name_instance = c_str_or_null(config.name_instance);
    opts->max_queued_calls = encode_max_queued_calls(config.max_queued_calls);
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

/// 缓存声明 → AmToolOptions / AmResourceOptions 的 cache_ttl_ms / cache_scope（v22；未声明为 0）。
/// @error 声明了缓存但 ttl_ms 为 0（C ABI 中 0 表示未声明，无法表达）→ Error(AM_ERR_INVALID_CONFIG)；其余范围由原生库校验。
template <class Options>
inline void set_cache(Options& o, const std::optional<CachePolicy>& cache) {
    if (cache && cache->ttl_ms == 0) throw Error(AM_ERR_INVALID_CONFIG, "cache.ttl_ms 须在 1..=86400000 之间（为 0）");
    o.cache_ttl_ms = cache ? cache->ttl_ms : 0;
    o.cache_scope = static_cast<int>(cache ? cache->scope : CacheScope::Private);
}

/// ToolOptions → AmToolOptions。
/// @invariant 指针借用 annotations_json、implements（均由调用方保持存活）与 options 的字符串。
inline AmToolOptions tool_options(const ToolOptions& options, const std::optional<std::string>& annotations_json,
                                  std::vector<const char*>& implements) {
    implements.clear();
    implements.reserve(options.implements.size());
    for (const auto& verb : options.implements) implements.push_back(verb.c_str());
    AmToolOptions o{};
    o.struct_size = sizeof(AmToolOptions);
    o.annotations_json = c_str_or_null(annotations_json);
    o.output_schema_json = c_str_or_null(options.output_schema_json);
    o.page = c_str_or_null(options.page);
    o.surface = static_cast<int>(options.surface);
    o.background_tool = c_str_or_null(options.background_tool);
    o.concurrency = options.concurrency;
    o.exclusive = c_str_or_null(options.exclusive);
    o.implements = implements.empty() ? nullptr : implements.data();
    o.implements_len = implements.size();
    set_cache(o, options.cache);
    if (options.deprecated) {
        o.deprecated_message = options.deprecated->message.c_str();
        o.deprecated_replacement = c_str_or_null(options.deprecated->replacement);
        o.deprecated_until = c_str_or_null(options.deprecated->until);
    }
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

}  // namespace app_mcp

#endif  // APP_MCP_TYPES_HPP
