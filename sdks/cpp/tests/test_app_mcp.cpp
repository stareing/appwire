// C++ 封装的测试程序（不依赖测试框架）。
//
// 第一部分不需要原生运行时；第二部分需要原生运行时已实现（未实现时 am_client_new 返回
// AM_ERR_PANIC，这部分会被跳过并打印提示）。都不需要 Host。

#include <chrono>
#include <condition_variable>
#include <cstdio>
#include <cstring>
#include <functional>
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

}  // namespace

int main() {
    try {
        test_basics();
        test_runtime();
        test_lifecycle();
        test_diagnostics();
    } catch (const std::exception& e) {
        ++g_failed;
        std::fprintf(stderr, "FAIL 未捕获的异常：%s\n", e.what());
    }
    std::fprintf(stderr, "%d passed, %d failed\n", g_passed, g_failed);
    return g_failed == 0 ? 0 : 1;
}
