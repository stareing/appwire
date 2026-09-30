// lifecycle_cpp.cpp —— C++ 封装的生命周期集成测试（对真实 fake_host 往返）。
//
// 用法：lifecycle_cpp_test <fake_host 可执行文件>
// 流程与 lifecycle_c.c 相同，但走 RAII 封装：ToolCallError 带 details、Call::hold()、
// Client::handle_wake(argc, argv)、on_idle_exit。
#include <atomic>
#include <chrono>
#include <cstdio>
#include <cstring>
#include <string>
#include <thread>

#include "app_mcp.hpp"

#ifdef _WIN32
#define popen _popen
#define pclose _pclose
#else
#include <sys/wait.h>
#endif

namespace {

int g_failed = 0;
#define EXPECT(cond)                                                                 \
    do {                                                                             \
        if (!(cond)) {                                                               \
            ++g_failed;                                                              \
            std::fprintf(stderr, "FAIL %s:%d: %s\n", __FILE__, __LINE__, #cond);     \
        }                                                                            \
    } while (0)

std::string extract_arg(const std::string& line) {
    const std::string key = "\"arg\":\"";
    auto p = line.find(key);
    if (p == std::string::npos) return {};
    p += key.size();
    auto end = line.find('"', p);
    return end == std::string::npos ? std::string() : line.substr(p, end - p);
}

}  // namespace

int main(int argc, char** argv) {
    if (argc < 2) {
        std::fprintf(stderr, "用法：%s <fake_host>\n", argv[0]);
        return 2;
    }
    std::string cmd = std::string("\"") + argv[1] +
                      "\" --addr 127.0.0.1:0 --invoke reject_tool --await-sleep --wake "
                      "--invoke greet --await-sleep --timeout-ms 20000";
    FILE* host = popen(cmd.c_str(), "r");
    if (!host) return 1;

    char buf[8192];
    std::string url;
    if (std::fgets(buf, sizeof buf, host) && std::strncmp(buf, "LISTENING ", 10) == 0) {
        std::string addr(buf + 10);
        addr.erase(addr.find_last_not_of("\r\n") + 1);
        url = "ws://" + addr;
    }
    if (url.empty()) {
        std::fprintf(stderr, "FAIL fake_host 没有输出 LISTENING\n");
        pclose(host);
        return 1;
    }

    std::atomic<int> idle_exits{0};
    std::atomic<int> held{0};
    int rc = 1;
    {
        app_mcp::ClientConfig config;
        config.app_id = "lifecycle-cpp";
        config.app_name = "Lifecycle C++";
        config.host_url = url;
        config.connect_timeout_ms = 2000;
        config.lifecycle.mode = AM_LIFECYCLE_IDLE;
        config.lifecycle.idle_timeout_ms = 300;
        config.lifecycle.residency = AM_RESIDENCY_EXIT_ALWAYS;
        config.lifecycle.wake = app_mcp::WakeDescriptor{AM_WAKE_URI, std::string("lifecycle-cpp"), true};

        app_mcp::ClientCallbacks callbacks;
        callbacks.on_idle_exit = [&] { ++idle_exits; };
        app_mcp::Client client(config, callbacks);

        auto t1 = client.register_tool(
            "reject_tool", "总是拒绝", [&](app_mcp::Call call) {
                {
                    app_mcp::HoldGuard h = call.hold();
                    if (h) ++held;
                }  // 析构即释放
                throw app_mcp::ToolCallError("USER_REJECTED", "额度不足", std::string(R"({"hint":"upgrade"})"));
            });
        auto t2 = client.register_tool("greet", "问好",
                                       [](app_mcp::Call call) { call.complete(R"({"greeting":"Hello, World!"})"); });
        client.start();

        bool saw_wake = false, hello_current = false, unsynced = false, greet_ok = false, details_ok = false;
        int sleeps = 0;
        while (std::fgets(buf, sizeof buf, host)) {
            std::string line(buf);
            std::fprintf(stderr, "[fake_host] %s", buf);
            auto has = [&](const char* s) { return line.find(s) != std::string::npos; };
            if (has("\"type\":\"wake\"")) {
                EXPECT(client.state().status == AM_STATE_DORMANT);
                std::string arg = extract_arg(line);
                // 模拟 main(argc, argv)：第一个参数不是唤醒参数。
                const char* args[] = {"lifecycle_cpp", "--verbose", arg.c_str()};
                EXPECT(client.handle_wake(3, args));
                saw_wake = true;
            } else if (has("\"type\":\"hello\"")) {
                hello_current = has("\"toolsCurrent\":true");
            } else if (has("\"type\":\"tools\"") && has("\"synced\":false")) {
                unsynced = true;
            } else if (has("\"type\":\"sleep\"") && has("\"accepted\":true")) {
                ++sleeps;
            } else if (has("\"type\":\"invoke\"")) {
                if (has("Hello, World!")) greet_ok = true;
                if (has("USER_REJECTED") && has("\"hint\":\"upgrade\"")) details_ok = true;
            }
        }
        int status = pclose(host);
#ifndef _WIN32
        status = WIFEXITED(status) ? WEXITSTATUS(status) : -1;
#endif
        EXPECT(status == 0);
        EXPECT(saw_wake);
        EXPECT(hello_current);
        EXPECT(unsynced);
        EXPECT(sleeps == 2);
        EXPECT(greet_ok);
        EXPECT(details_ok);
        EXPECT(held == 1);
        for (int i = 0; i < 50 && idle_exits < 2; ++i) std::this_thread::sleep_for(std::chrono::milliseconds(50));
        EXPECT(idle_exits == 2);
        EXPECT(client.state().status == AM_STATE_DORMANT);
        EXPECT(client.tools_hash().size() == 16);
        rc = g_failed == 0 ? 0 : 1;
    }
    std::fprintf(stderr, rc == 0 ? "lifecycle_cpp: PASS\n" : "lifecycle_cpp: FAIL\n");
    return rc;
}
