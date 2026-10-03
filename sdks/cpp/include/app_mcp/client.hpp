// app_mcp/client.hpp —— Tool / Resource / Scope / Client（由 app_mcp.hpp 包含；直接包含 app_mcp.hpp 即可）。
#ifndef APP_MCP_CLIENT_HPP
#define APP_MCP_CLIENT_HPP

#include "handlers.hpp"

namespace app_mcp {

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
        return register_resource(name, description, std::move(reader), ResourceOptions{mime_type, false, std::nullopt});
    }

    Resource register_resource(const std::string& name, const std::string& description, ResourceReader reader,
                               const ResourceOptions& options) {
        AmResourceSpec spec{};
        spec.name = name.c_str();
        spec.description = description.c_str();
        spec.mime_type = detail::c_str_or_null(options.mime_type);
        std::optional<std::string> annotations;
        if (options.annotations) annotations = detail::to_json(*options.annotations);
        AmResourceOptions ropts{};
        ropts.struct_size = sizeof(AmResourceOptions);
        ropts.realtime = options.realtime;
        ropts.annotations_json = detail::c_str_or_null(annotations);
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
        // @why AmClientOptions 不含 busy 策略（app_mcp.h v19 结构体不变），创建后立即设置。
        if (AmStatus s = am_client_set_busy_policy(h_, config.busy_policy); s != AM_OK) {
            const char* msg = am_last_error_message();
            std::string message = msg ? msg : "";
            am_client_free(std::exchange(h_, nullptr));
            throw Error(s, message);
        }
        busy_->client = h_;
    }
    // @invariant busy_ 随句柄一起移动：未结束的 BusyScope 引用同一份状态。
    Client(Client&& o) noexcept : h_(std::exchange(o.h_, nullptr)), busy_(std::exchange(o.busy_, nullptr)) {}
    Client& operator=(Client&& o) noexcept {
        if (this != &o) {
            release();
            h_ = std::exchange(o.h_, nullptr);
            busy_ = std::exchange(o.busy_, nullptr);
        }
        return *this;
    }
    Client(const Client&) = delete;
    Client& operator=(const Client&) = delete;
    /// 停止并释放客户端（阻塞到后台线程结束）。不要在回调线程上析构。
    ~Client() { release(); }

    AmClient* get() const noexcept { return h_; }

    void start() { detail::check(am_client_start(h_)); }
    void stop() { detail::check(am_client_stop(h_)); }
    void set_visibility(Visibility visibility, bool focused) {
        detail::check(am_client_set_visibility(h_, visibility, focused));
    }
    /// App 在后台（Hidden / Frozen）时是否仍把导航交给导航回调（app_mcp.h v15）；false 时直接以
    /// USER_ACTION_REQUIRED（reason "foreground"）回复。默认随平台：桌面 true，Android / iOS / 鸿蒙 false。
    void set_navigate_in_background(bool enabled) { detail::check(am_client_set_navigate_in_background(h_, enabled)); }

    // ---- 用户正在操作（spec/protocol.md 5.3，app_mcp.h v19） ----

    /// 显式开关：声明用户正在 / 不再在 App 内操作。期间写调用（生效注解不是 readOnlyHint: true 的工具）按 busy_policy
    /// 拒绝或排队；只读调用与已开始的调用不受影响。何时算"正在操作"由 App 决定，随时生效。
    /// 有效 busy = 本开关 ∨ 未结束的 busy() 作用域数 > 0：set_busy(false) 不结束进行中的作用域。
    void set_busy(bool busy) {
        if (!h_) throw Error(AM_ERR_INVALID_ARGUMENT, "客户端已移走");
        std::lock_guard<std::mutex> lock(busy_->mu);
        busy_->manual = busy;
        detail::check(busy_->push());
    }
    /// 有效 busy（显式开关 ∨ 作用域数 > 0）。
    bool is_busy() const {
        if (!h_) throw Error(AM_ERR_INVALID_ARGUMENT, "客户端已移走");
        return busy_->effective();
    }
    /// 修改用户正在操作期间写调用的处理方式；随即对排队中的调用生效（改为 AM_BUSY_REJECT 时排队的写调用被拒绝）。
    void set_busy_policy(BusyPolicy policy) { detail::check(am_client_set_busy_policy(h_, policy)); }
    /// 开始一个用户正在操作的作用域（引用计数 +1，从 0 变 1 且开关为 false 时下发 busy）；作用域结束时 -1。
    [[nodiscard]] BusyScope busy() {
        if (!h_) throw Error(AM_ERR_INVALID_ARGUMENT, "客户端已移走");
        std::lock_guard<std::mutex> lock(busy_->mu);
        ++busy_->scopes;
        if (AmStatus s = busy_->push(); s != AM_OK) {
            --busy_->scopes;
            detail::check(s);
        }
        return BusyScope(busy_);
    }

    // ---- 事件（spec/protocol.md 3.5，app_mcp.h v20） ----

    /// 声明本实例可发出的事件（同名替换）。已连接时随即同步给 Host，否则在下次握手成功后同步；不触发连接。
    /// SDK 不读清单：要发出的事件都需在运行时声明。payload_schema_json 为载荷的 JSON Schema（对象文本，描述用）。
    /// @error 名称不合法 → AM_ERR_INVALID_NAME；schema 不是 JSON 对象 → AM_ERR_INVALID_SCHEMA；已停止 → AM_ERR_STOPPED。
    void declare_event(const std::string& name, const std::string& description,
                       const std::optional<std::string>& payload_schema_json = std::nullopt) {
        detail::check(am_client_declare_event(h_, name.c_str(), description.c_str(),
                                              payload_schema_json ? payload_schema_json->c_str() : nullptr));
    }
    /// 撤销事件声明；返回是否撤销了已有声明（未声明过为 false）。
    bool remove_event(const std::string& name) {
        bool removed = false;
        detail::check(am_client_remove_event(h_, name.c_str(), &removed));
        return removed;
    }
    /// 发出已声明的事件；payload_json 为 JSON 对象文本（nullopt = 无载荷）。已连接时发送并返回 true；未连接（休眠、断线、
    /// 重连中、握手中）时丢弃并返回 false——不缓存、不为此连接或唤醒 Host。需要可靠送达的状态变化请改用资源。
    /// @error 未声明 / 名称不合法 → AM_ERR_INVALID_NAME；载荷不是 JSON 对象或超过 8 KiB → AM_ERR_INVALID_JSON。
    bool emit_event(const std::string& name, const std::optional<std::string>& payload_json = std::nullopt) {
        bool sent = false;
        detail::check(am_client_emit_event(h_, name.c_str(), payload_json ? payload_json->c_str() : nullptr, &sent));
        return sent;
    }

    /// 设置导航回调（spec/protocol.md 3.4）；传空的 std::function 清除（之后的导航请求以 NAVIGATION_FAILED 回复）。
    /// 能力在握手时声明：建议在 start() 之前设置，连接后才设置的在下次连接时生效。handler 在分发线程上调用。
    void set_navigation_handler(NavigationHandler handler) {
        if (!handler) {
            detail::check(am_client_set_navigation_handler(h_, nullptr, nullptr, nullptr));
            return;
        }
        // 所有权交给库：替换 / 清除 / 释放客户端时库调用 delete_fn。
        auto* holder = new NavigationHandler(std::move(handler));
        AmStatus s = am_client_set_navigation_handler(h_, &detail::navigate_trampoline, holder,
                                                      &detail::delete_fn<NavigationHandler>);
        detail::check(s);
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
    /// 释放句柄；先断开 busy 状态，之后结束的 BusyScope 不再访问已释放的句柄。
    void release() noexcept {
        if (busy_) {
            std::lock_guard<std::mutex> lock(busy_->mu);
            busy_->client = nullptr;
        }
        am_client_free(std::exchange(h_, nullptr));
    }

    AmClient* h_ = nullptr;
    std::shared_ptr<detail::BusyState> busy_ = std::make_shared<detail::BusyState>();
};

inline std::string version() { return am_version(); }

/// 从激活参数中提取唤醒令牌（不需要客户端；如单实例转发前判断参数是否为唤醒）。
inline std::optional<std::string> parse_wake_token(const std::string& args) {
    char* t = am_parse_wake_token(args.c_str());
    if (!t) return std::nullopt;
    return detail::take_string(t);
}

}  // namespace app_mcp

#endif  // APP_MCP_CLIENT_HPP
