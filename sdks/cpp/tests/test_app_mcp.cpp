// C++ 封装的测试程序（不依赖测试框架）。
//
// 第一部分不需要原生运行时；第二部分需要原生运行时已实现（未实现时 am_client_new 返回
// AM_ERR_PANIC，这部分会被跳过并打印提示）。都不需要 Host。

#include <chrono>
#include <condition_variable>
#include <cstdio>
#include <cstring>
#include <functional>
#include <limits>
#include <memory>
#include <mutex>
#include <optional>
#include <string>

#include "app_mcp.hpp"

static_assert(AM_API_VERSION >= 3, "生命周期 API 需要 API 版本 3");
// @why 回归：MSVC 未加 /utf-8 时按系统代码页编译，中文字面量不是 UTF-8（CMakeLists.txt 经 app_mcp 目标传递 /utf-8）。
static_assert(sizeof("中") == 4, "中文字面量须按 UTF-8 编码（MSVC 需 /utf-8）");

namespace {

int g_failed = 0;
int g_passed = 0;

#define EXPECT(cond)                                                          \
    do {                                                                      \
        if (cond) {                                                           \
            ++g_passed;                                                       \
        } else {                                                              \
            ++g_failed;                                                       \
            std::fprintf(stderr, "FAIL %s:%d: %s\n", __FILE__, __LINE__, #cond); \
        }                                                                     \
    } while (0)

AmStatus status_of(const std::function<void()>& f) {
    try {
        f();
    } catch (const app_mcp::Error& e) {
        return e.status();
    }
    return AM_OK;
}

void test_basics() {
    EXPECT(!app_mcp::version().empty());
    EXPECT(app_mcp::json_quote("a\"b\\c\n") == "\"a\\\"b\\\\c\\n\"");
    EXPECT(app_mcp::json_quote(std::string("\x01", 1)) == "\"\\u0001\"");

    // 空句柄 → AM_ERR_INVALID_ARGUMENT，并带错误信息。
    app_mcp::Scope empty;
    try {
        empty.create_scope("x");
        EXPECT(false);
    } catch (const app_mcp::Error& e) {
        EXPECT(e.status() == AM_ERR_INVALID_ARGUMENT);
        EXPECT(std::strlen(e.what()) > 0);
    }
    app_mcp::Tool tool;
    EXPECT(status_of([&] { tool.set_enabled(false); }) == AM_ERR_INVALID_ARGUMENT);
    app_mcp::Resource res;
    EXPECT(status_of([&] { res.notify_changed(); }) == AM_ERR_INVALID_ARGUMENT);

    // 注册失败时 handler 由库释放（捕获的 shared_ptr 引用计数恢复）。
    auto token = std::make_shared<int>(1);
    EXPECT(status_of([&] { empty.register_tool("t", "d", [token](app_mcp::Call) {}); }) == AM_ERR_INVALID_ARGUMENT);
    EXPECT(token.use_count() == 1);

    // C 层：状态码与头文件一致。
    EXPECT(am_call_complete(nullptr, "null", nullptr, 0) == AM_ERR_INVALID_ARGUMENT);
    EXPECT(am_read_fail(nullptr, "HANDLER_ERROR", "x") == AM_ERR_INVALID_ARGUMENT);
    EXPECT(am_read_fail_with_details(nullptr, "HANDLER_ERROR", "x", "{}") == AM_ERR_INVALID_ARGUMENT);
    EXPECT(am_read_fail_user_action(nullptr, "x", nullptr, nullptr) == AM_ERR_INVALID_ARGUMENT);
}

void test_runtime() {
    app_mcp::ClientConfig config;
    config.app_id = "cpp-test";
    config.app_name = "C++ Test";
    config.host_url = "ws://127.0.0.1:1";  // 不会 start，不连接

    AmClient* probe = nullptr;
    AmClientConfig c{};
    c.app_id = config.app_id.c_str();
    c.app_name = config.app_name.c_str();
    c.host_url = config.host_url->c_str();
    AmStatus s = am_client_new(&c, nullptr, &probe);
    if (s == AM_ERR_PANIC) {
        std::fprintf(stderr, "SKIP 运行时测试：原生运行时尚未实现（%s）\n", am_last_error_message());
        return;
    }
    EXPECT(s == AM_OK);
    am_client_free(probe);

    app_mcp::Client client(config);
    EXPECT(client.state().status == AM_STATE_IDLE);
    EXPECT(!client.instance_id().empty());
    EXPECT(!client.token().has_value());

    // 带总览创建。
    {
        app_mcp::ClientConfig with_overview = config;
        with_overview.overview = app_mcp::AppOverview{"示例 App", std::string("## 能力\n- 问好"), std::string("zh-CN")};
        EXPECT(status_of([&] { app_mcp::Client c2(with_overview); }) == AM_OK);
    }

    auto handler = [](app_mcp::Call call) { call.complete("null"); };
    auto t = client.register_tool("echo", "回显", handler);
    EXPECT(static_cast<bool>(t));
    EXPECT(status_of([&] { client.register_tool("echo", "重名", handler); }) == AM_ERR_DUPLICATE_NAME);
    EXPECT(status_of([&] { client.register_tool("bad name!", "非法", handler); }) == AM_ERR_INVALID_NAME);

    app_mcp::ToolOptions bad_schema;
    bad_schema.input_schema_json = R"({"type":"string"})";
    EXPECT(status_of([&] { client.register_tool("s1", "schema", handler, bad_schema); }) == AM_ERR_INVALID_SCHEMA);
    bad_schema.input_schema_json = "{not json";
    AmStatus st = status_of([&] { client.register_tool("s2", "schema", handler, bad_schema); });
    EXPECT(st == AM_ERR_INVALID_SCHEMA || st == AM_ERR_INVALID_JSON);

    EXPECT(status_of([&] { t.set_enabled(false); }) == AM_OK);
    EXPECT(status_of([&] { t.update("新描述"); }) == AM_OK);

    // Scope：dispose 后同名工具可重新注册。
    auto scope = client.create_scope("page");
    auto t2 = scope.register_tool("page.action", "页面动作", handler);
    EXPECT(status_of([&] { scope.dispose(); }) == AM_OK);
    EXPECT(status_of([&] { scope.dispose(); }) == AM_OK);  // 幂等
    auto t3 = client.register_tool("page.action", "页面动作", handler);
    EXPECT(static_cast<bool>(t3));

    auto r = client.register_resource("app.state", "状态", [](app_mcp::Read read) { read.complete("{}"); });
    EXPECT(status_of([&] { r.notify_changed(); }) == AM_OK);
    EXPECT(status_of([&] { client.register_resource("app.state", "重名", [](app_mcp::Read) {}); }) ==
           AM_ERR_DUPLICATE_NAME);

    // 根作用域 dispose 注销全部。
    auto root = client.root_scope();
    EXPECT(status_of([&] { root.dispose(); }) == AM_OK);
    auto again = client.register_tool("echo", "回显", handler);
    EXPECT(static_cast<bool>(again));

    EXPECT(status_of([&] { client.set_visibility(AM_HIDDEN, false); }) == AM_OK);
    EXPECT(status_of([&] { client.stop(); }) == AM_OK);
    EXPECT(status_of([&] { client.register_tool("after.stop", "x", handler); }) == AM_ERR_STOPPED);
}

void test_lifecycle() {
    // 不需要客户端的部分。
    EXPECT(app_mcp::parse_wake_token("myapp://app-mcp/wake?token=t-1") == std::optional<std::string>("t-1"));
    EXPECT(app_mcp::parse_wake_token("app-mcp-wake:abc") == std::optional<std::string>("abc"));
    EXPECT(!app_mcp::parse_wake_token("--flag").has_value());

    app_mcp::HoldGuard empty_hold;
    EXPECT(!empty_hold);
    empty_hold.release();  // 空句柄释放无效果

    app_mcp::ToolCallError with_details("USER_REJECTED", "x", std::string("{\"a\":1}"));
    EXPECT(with_details.details_json().has_value());
    EXPECT(!app_mcp::ToolCallError("HANDLER_ERROR", "y").details_json().has_value());

    EXPECT(am_call_fail_with_details(nullptr, "HANDLER_ERROR", "x", "{}") == AM_ERR_INVALID_ARGUMENT);

    // v11：UserActionRequired 是 kind 为 USER_ACTION_REQUIRED 的 ToolCallError
    app_mcp::UserActionRequired ua("请先登录", std::string(app_mcp::user_action_reason::login));
    const app_mcp::ToolCallError& as_tool_error = ua;
    EXPECT(as_tool_error.kind() == "USER_ACTION_REQUIRED");
    EXPECT(ua.reason() == std::optional<std::string>("login"));
    EXPECT(!ua.uri().has_value());
    EXPECT(std::string(ua.what()) == "请先登录");
    EXPECT(am_call_fail_user_action(nullptr, "x", nullptr, nullptr) == AM_ERR_INVALID_ARGUMENT);

    AmLifecycle lc;
    std::memset(&lc, 0xff, sizeof lc);
    am_lifecycle_init(&lc);
    EXPECT(lc.mode == AM_LIFECYCLE_PERSISTENT && lc.idle_timeout_ms == 60000 && lc.hidden_idle_timeout_ms == 15000 &&
           lc.grace_ms == 10000 && lc.residency == AM_RESIDENCY_KEEP && lc.wake_kind == AM_WAKE_UNSET &&
           lc.wake_target == nullptr);

    app_mcp::ClientConfig config;
    config.app_id = "cpp-lifecycle";
    config.app_name = "C++ Lifecycle";
    config.host_url = "ws://127.0.0.1:1";
    config.connect_timeout_ms = 200;
    config.lifecycle.mode = AM_LIFECYCLE_ON_DEMAND;
    config.lifecycle.hidden_idle_timeout_ms = 0;
    config.lifecycle.wake = app_mcp::WakeDescriptor{AM_WAKE_AUMID, std::string("Co.App_abc!App"), false};

    // 非法枚举值 → AM_ERR_INVALID_ARGUMENT。
    {
        app_mcp::ClientConfig bad = config;
        bad.lifecycle.residency = static_cast<AmResidency>(7);
        EXPECT(status_of([&] { app_mcp::Client c(bad); }) == AM_ERR_INVALID_ARGUMENT);
    }

    app_mcp::ClientCallbacks callbacks;
    int idle_exits = 0;
    callbacks.on_idle_exit = [&] { ++idle_exits; };
    app_mcp::Client client(config, callbacks);
    client.start();
    EXPECT(client.state().status == AM_STATE_DORMANT);  // on-demand：启动时不连接

    std::string h1 = client.tools_hash();
    EXPECT(h1.size() == 16);
    auto t = client.register_tool("echo", "回显", [](app_mcp::Call call) { call.complete(); });
    EXPECT(client.tools_hash() != h1);  // 休眠中注册只更新摘要

    {
        app_mcp::HoldGuard a = client.hold();
        EXPECT(a.active());
        app_mcp::HoldGuard b = std::move(a);
        EXPECT(!a.active() && b.active());
        b.release();
        EXPECT(!b.active());
    }

    EXPECT(!client.handle_wake("--unrelated"));
    const char* argv_like[] = {"app", "--x"};
    EXPECT(!client.handle_wake(2, argv_like));
    EXPECT(!client.sleep());  // 已在休眠
    EXPECT(client.connect_now());
    EXPECT(status_of([&] { client.wake(static_cast<AmWakeReason>(42)); }) == AM_ERR_INVALID_ARGUMENT);
    EXPECT(status_of([&] { client.sleep(static_cast<AmSleepReason>(-2)); }) == AM_ERR_INVALID_ARGUMENT);
    EXPECT(client.handle_wake("app-mcp-wake:tok-1"));
    client.stop();
    EXPECT(idle_exits == 0);
}

void test_power_options() {
    // 默认值等于核心默认（spec/lifecycle.md 第 3 节）：C ABI 编码 0 = 默认。
    app_mcp::ClientConfig config;
    config.app_id = "cpp-power";
    config.app_name = "C++ Power";
    EXPECT(config.lifecycle.mode == AM_LIFECYCLE_PERSISTENT);
    EXPECT(config.heartbeat == AM_HEARTBEAT_AUTO);
    EXPECT(config.lifecycle.host_absent_retries == 3 && !config.lifecycle.legacy_timers &&
           config.lifecycle.merge_window_ms == 2000 && !config.lifecycle.sleep_on_background);
    AmLifecycle lc{};
    AmClientOptions opts{};
    app_mcp::detail::fill_client_options(config, &lc, &opts);
    EXPECT(opts.struct_size == sizeof(AmClientOptions) && opts.lifecycle == &lc);
    EXPECT(opts.heartbeat == AM_HEARTBEAT_AUTO && opts.host_absent_retries == 3 && !opts.legacy_timers &&
           opts.merge_window_ms == 2000 && !opts.sleep_on_background);
    // 调用去重（v13）：默认 5 分钟 / 64 条，原样传给 C ABI。
    EXPECT(config.call_dedup.ttl_ms == 300000 && config.call_dedup.max_entries == 64);
    EXPECT(opts.call_dedup_ttl_ms == 300000 && opts.call_dedup_max_entries == 64);

    // 显式值逐项映射；0 = 一直重连 / 不留窗口 → C ABI 负数。
    config.heartbeat = AM_HEARTBEAT_OFF;
    config.lifecycle.mode = AM_LIFECYCLE_IDLE;
    config.lifecycle.host_absent_retries = 0;
    config.lifecycle.legacy_timers = true;
    config.lifecycle.merge_window_ms = 0;
    config.lifecycle.sleep_on_background = true;
    config.call_dedup.ttl_ms = 0;
    app_mcp::detail::fill_client_options(config, &lc, &opts);
    EXPECT(lc.mode == AM_LIFECYCLE_IDLE);
    EXPECT(opts.call_dedup_ttl_ms < 0 && opts.call_dedup_max_entries == 64);  // 0 = 关闭 → C ABI 负数
    config.call_dedup = app_mcp::CallDedup{1000, 0};
    app_mcp::detail::fill_client_options(config, &lc, &opts);
    EXPECT(opts.call_dedup_ttl_ms == 1000 && opts.call_dedup_max_entries < 0);
    config.call_dedup = app_mcp::CallDedup{};
    EXPECT(opts.heartbeat == AM_HEARTBEAT_OFF && opts.host_absent_retries < 0 && opts.legacy_timers &&
           opts.merge_window_ms < 0 && opts.sleep_on_background);
    config.heartbeat = AM_HEARTBEAT_ALWAYS;
    config.lifecycle.host_absent_retries = 7;
    config.lifecycle.merge_window_ms = 500;
    app_mcp::detail::fill_client_options(config, &lc, &opts);
    EXPECT(opts.heartbeat == AM_HEARTBEAT_ALWAYS && opts.host_absent_retries == 7 && opts.merge_window_ms == 500);

    // 超出 C ABI 范围时截断而不是回绕成负数（负数在 C ABI 中另有含义）。
    EXPECT(app_mcp::detail::encode_host_absent_retries(UINT32_MAX) == INT32_MAX);
    EXPECT(app_mcp::detail::encode_merge_window_ms(UINT64_MAX) == INT64_MAX);
    EXPECT(app_mcp::detail::encode_dedup_ttl_ms(UINT64_MAX) == INT64_MAX);
    EXPECT(app_mcp::detail::encode_dedup_max_entries(UINT32_MAX) == INT32_MAX);

    // 非法心跳枚举值 → AM_ERR_INVALID_ARGUMENT（说明字段确实传到了库）。
    config.host_url = "ws://127.0.0.1:1";
    {
        app_mcp::ClientConfig bad = config;
        bad.heartbeat = static_cast<AmHeartbeatMode>(9);
        EXPECT(status_of([&] { app_mcp::Client c(bad); }) == AM_ERR_INVALID_ARGUMENT);
    }

    // 带新字段创建客户端；realtime 资源注册（含与普通资源同名冲突）。
    app_mcp::Client client(config);
    app_mcp::ResourceOptions rt;
    rt.realtime = true;
    auto r1 = client.register_resource("order.status", "订单状态", [](app_mcp::Read read) { read.complete("{}"); }, rt);
    EXPECT(static_cast<bool>(r1));
    EXPECT(status_of([&] { r1.notify_changed(); }) == AM_OK);
    app_mcp::ResourceOptions text;
    text.mime_type = std::string("text/plain");
    auto r2 = client.register_resource("app.note", "备注", [](app_mcp::Read read) { read.complete("\"x\""); }, text);
    EXPECT(static_cast<bool>(r2));
    // 资源内容标注（v13）。
    app_mcp::ResourceOptions annotated;
    annotated.annotations = app_mcp::ContentAnnotations{};
    annotated.annotations->audience = std::vector<app_mcp::Audience>{app_mcp::Audience::User};
    annotated.annotations->priority = 0.5;
    auto r3 = client.register_resource("app.summary", "摘要", [](app_mcp::Read read) { read.complete("{}"); }, annotated);
    EXPECT(static_cast<bool>(r3));
    EXPECT(status_of([&] {
               client.register_resource("order.status", "重名", [](app_mcp::Read) {}, rt);
           }) == AM_ERR_DUPLICATE_NAME);
    client.stop();
}

void test_diagnostics() {
    // 不存在的本地 IPC 端点 → BACKOFF，错误码 HOST_NOT_RUNNING（spec/protocol.md 10.1）。
    // @why 不用 ws://127.0.0.1:1：WSL 等环境下回环连接未监听端口可能超时（CONNECT_TIMEOUT）而非被拒绝。
    app_mcp::ClientConfig config;
    config.app_id = "cpp-diag";
    config.app_name = "C++ Diagnostics";
#ifdef _WIN32
    config.host_url = "pipe:\\\\.\\pipe\\app-mcp-cpp-test-missing";
#else
    config.host_url = "unix:/nonexistent-app-mcp-cpp-test/hub.sock";
#endif
    config.connect_timeout_ms = 2000;

    std::mutex mu;
    std::condition_variable cv;
    std::optional<app_mcp::StateInfo> backoff;
    bool non_backoff_had_code = false;
    app_mcp::ClientCallbacks callbacks;
    callbacks.on_state = [&](const app_mcp::StateInfo& info) {
        std::lock_guard<std::mutex> lock(mu);
        if (info.status == AM_STATE_BACKOFF) {
            if (!backoff) backoff = info;
            cv.notify_all();
        } else if (info.status != AM_STATE_REJECTED && info.status != AM_STATE_HOST_MISMATCH && info.code) {
            non_backoff_had_code = true;
        }
    };
    app_mcp::Client client(config, callbacks);
    EXPECT(!client.state().code.has_value());  // IDLE 不带码
    EXPECT(!client.connection_id().has_value());
    client.start();
    {
        std::unique_lock<std::mutex> lock(mu);
        cv.wait_for(lock, std::chrono::seconds(10), [&] { return backoff.has_value(); });
        EXPECT(backoff.has_value());
        if (backoff) {
            EXPECT(backoff->code == std::optional<std::string>("HOST_NOT_RUNNING"));
            EXPECT(!backoff->reason.empty());
        }
        EXPECT(!non_backoff_had_code);
    }
    app_mcp::StateInfo now = client.state();
    if (now.status == AM_STATE_BACKOFF) {
        EXPECT(now.code == std::optional<std::string>("HOST_NOT_RUNNING"));
    }
    EXPECT(!client.connection_id().has_value());  // 从未连上
    client.stop();
    EXPECT(!client.state().code.has_value());  // STOPPED 不带码

    // 空句柄：查询函数报 AM_ERR_INVALID_ARGUMENT（经 detail::check 抛出）。
    app_mcp::Client moved = std::move(client);
    EXPECT(status_of([&] { (void)client.connection_id(); }) == AM_ERR_INVALID_ARGUMENT);
    EXPECT(status_of([&] { (void)client.state(); }) == AM_ERR_INVALID_ARGUMENT);
}

/// 工具注解 / 内容注解的 JSON 编码（app_mcp.h v9）；注册时库会再按协议类型校验。
void test_annotations() {
    app_mcp::ToolAnnotations ta;
    EXPECT(app_mcp::detail::to_json(ta) == "{}");
    ta.title = "下\"单";
    ta.read_only_hint = false;
    ta.open_world_hint = true;
    // @why MSVC 对宏参数中含反斜杠的原始字符串字面量做 # 字符串化时报 C2017，先放进变量
    const std::string expected_ta = R"({"title":"下\"单","readOnlyHint":false,"openWorldHint":true})";
    EXPECT(app_mcp::detail::to_json(ta) == expected_ta);

    app_mcp::ContentAnnotations ca;
    ca.audience = std::vector<app_mcp::Audience>{app_mcp::Audience::User, app_mcp::Audience::Assistant};
    ca.priority = 0.5;
    ca.last_modified = "2026-10-02T00:00:00Z";
    EXPECT(app_mcp::detail::to_json(ca) ==
           R"({"audience":["user","assistant"],"priority":0.5,"lastModified":"2026-10-02T00:00:00Z"})");
    ca = {};
    ca.priority = std::numeric_limits<double>::quiet_NaN();
    EXPECT(app_mcp::detail::to_json(ca) == "{}");

    app_mcp::ClientConfig config;
    config.app_id = "cpp-annotations";
    config.app_name = "C++ Annotations";
    config.host_url = "ws://127.0.0.1:1";  // 不会 start，不连接
    app_mcp::Client client(config);
    auto handler = [](app_mcp::Call call) { call.complete(app_mcp::CallResult{}); };
    app_mcp::ToolOptions options;
    options.annotations = app_mcp::ToolAnnotations{std::nullopt, true, std::nullopt, std::nullopt, std::nullopt};
    options.output_schema_json = R"({"type":"object"})";
    auto t = client.register_tool("with.options", "带选项", handler, options);
    EXPECT(static_cast<bool>(t));
    app_mcp::ToolOptions bad = options;
    bad.output_schema_json = "{";
    EXPECT(status_of([&] { client.register_tool("bad.output", "x", handler, bad); }) == AM_ERR_INVALID_SCHEMA);
    EXPECT(status_of([&] { t.update("改", bad); }) == AM_ERR_INVALID_SCHEMA);
    EXPECT(status_of([&] { t.update("改"); }) == AM_OK);  // 清除注解与 outputSchema
}

void test_navigation() {
    app_mcp::ClientConfig config;
    config.app_id = "cpp-navigation";
    config.app_name = "C++ Navigation";
    config.host_url = "ws://127.0.0.1:1";  // 不会 start，不连接
    app_mcp::Client client(config);
    auto handler = [](app_mcp::Call call) { call.complete(); };
    app_mcp::ToolOptions options;
    options.surface = app_mcp::Surface::View;
    options.page = "cart";
    auto t = client.register_tool("cart.checkout", "结算", handler, options);
    EXPECT(static_cast<bool>(t));
    EXPECT(app_mcp::detail::tool_options(options, std::nullopt).surface == AM_SURFACE_VIEW);
    EXPECT(std::strcmp(app_mcp::detail::tool_options(options, std::nullopt).page, "cart") == 0);
    EXPECT(app_mcp::detail::tool_options(app_mcp::ToolOptions{}, std::nullopt).page == nullptr);
    EXPECT(app_mcp::detail::tool_options(app_mcp::ToolOptions{}, std::nullopt).surface == AM_SURFACE_APP);
    // 声明影响 toolsHash；页面名非法时注册失败。
    std::string before = client.tools_hash();
    options.page = "orders";
    EXPECT(status_of([&] { t.update("结算", options); }) == AM_OK);
    EXPECT(client.tools_hash() != before);
    app_mcp::ToolOptions bad = options;
    bad.page = "bad page!";
    EXPECT(status_of([&] { client.register_tool("bad.page", "x", handler, bad); }) != AM_OK);

    // v15：backgroundTool 随声明传给 C 接口（影响 toolsHash），名称非法时更新失败；navigateInBackground 可设置。
    EXPECT(app_mcp::detail::tool_options(app_mcp::ToolOptions{}, std::nullopt).background_tool == nullptr);
    options.background_tool = "cart.summary";
    EXPECT(std::strcmp(app_mcp::detail::tool_options(options, std::nullopt).background_tool, "cart.summary") == 0);
    before = client.tools_hash();
    EXPECT(status_of([&] { t.update("结算", options); }) == AM_OK);
    EXPECT(client.tools_hash() != before);
    app_mcp::ToolOptions bad_bg = options;
    bad_bg.background_tool = "bad tool!";
    EXPECT(status_of([&] { t.update("结算", bad_bg); }) != AM_OK);
    EXPECT(status_of([&] { client.set_navigate_in_background(false); }) == AM_OK);
    EXPECT(status_of([&] { client.set_navigate_in_background(true); }) == AM_OK);

    // 设置 / 替换 / 清除导航回调；替换与清除时库释放旧的 std::function。
    auto alive = std::make_shared<int>(0);
    std::weak_ptr<int> watch = alive;
    client.set_navigation_handler([alive](app_mcp::Navigate nav) { nav.complete(); });
    alive.reset();
    EXPECT(!watch.expired());
    client.set_navigation_handler([](app_mcp::Navigate nav) { nav.deny("忙"); });
    EXPECT(watch.expired());
    EXPECT(status_of([&] { client.set_navigation_handler(nullptr); }) == AM_OK);

    // 未持有请求的 Navigate：完成函数抛出 ALREADY_COMPLETED，析构不做任何事。
    app_mcp::Navigate empty(std::make_shared<app_mcp::detail::Pending<AmNavigate>>(nullptr));
    EXPECT(!empty.pending());
    EXPECT(status_of([&] { empty.complete(); }) == AM_ERR_ALREADY_COMPLETED);
    EXPECT(status_of([&] { empty.deny("x"); }) == AM_ERR_ALREADY_COMPLETED);
    EXPECT(status_of([&] { empty.fail_user_action("x", "foreground"); }) == AM_ERR_ALREADY_COMPLETED);
}

void test_name_options() {
    // 按名寻址（app_mcp.h v17，spec/naming.md）：默认不登记；register_name / name_instance 原样传给 C ABI。
    app_mcp::ClientConfig config;
    config.app_id = "cpp-named";
    config.app_name = "C++ Named";
    config.host_url = "ws://127.0.0.1:1";
    AmLifecycle lc{};
    AmClientOptions opts{};
    app_mcp::detail::fill_client_options(config, &lc, &opts);
    EXPECT(!opts.register_name && opts.name_instance == nullptr);
    config.register_name = true;
    config.name_instance = std::string("w2");
    app_mcp::detail::fill_client_options(config, &lc, &opts);
    EXPECT(opts.register_name && opts.name_instance == config.name_instance->c_str());

    // 实例名的规则只在原生库定义（spec/naming.md 2.1）：不合法时构造抛出 AM_ERR_INVALID_CONFIG。
    for (const char* bad : {"default", "W2", "2w", ""}) {
        app_mcp::ClientConfig b = config;
        b.name_instance = std::string(bad);
        EXPECT(status_of([&] { app_mcp::Client c(b); }) == AM_ERR_INVALID_CONFIG);
    }
    // 合法时创建成功（不调用 start：不在名字服务登记）。
    config.lifecycle.mode = AM_LIFECYCLE_ON_DEMAND;
    config.lifecycle.residency = AM_RESIDENCY_EXIT_WHEN_IDLE;
    EXPECT(status_of([&] { app_mcp::Client c(config); }) == AM_OK);
}

}  // namespace

int main() {
    try {
        test_basics();
        test_runtime();
        test_lifecycle();
        test_power_options();
        test_name_options();
        test_diagnostics();
        test_annotations();
        test_navigation();
    } catch (const std::exception& e) {
        ++g_failed;
        std::fprintf(stderr, "FAIL 未捕获的异常：%s\n", e.what());
    }
    std::fprintf(stderr, "%d passed, %d failed\n", g_passed, g_failed);
    return g_failed == 0 ? 0 : 1;
}
