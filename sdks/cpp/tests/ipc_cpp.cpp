// ipc_cpp.cpp —— C++ 封装经本地 IPC 正向连接 fake_host 的集成测试。
//
// 用法：ipc_cpp_test <fake_host 可执行文件>
// 端点：Windows 为每进程独立的命名管道 pipe:\\.\pipe\app-mcp-cpp-test-<pid>，其他平台为临时目录下的
// Unix 域套接字；都不使用平台默认端点（常驻 Host 可能正在用）。
// 检查：连接成功、handler 内 connection_id 非空（fake_host 返回 "fake-<pid>"）、调用一次工具。
#include <atomic>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <mutex>
#include <optional>
#include <string>

#include "app_mcp.hpp"

#ifdef _WIN32
#include <process.h>
#define popen _popen
#define pclose _pclose
#else
#include <sys/wait.h>
#include <unistd.h>
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

/// 本次测试专用的 IPC 端点；`dir` 为需要在结束时删除的临时目录（Windows 为空）。
struct TestEndpoint {
    std::string endpoint;
    std::string dir;
};

std::optional<TestEndpoint> make_endpoint() {
#ifdef _WIN32
    return TestEndpoint{"pipe:\\\\.\\pipe\\app-mcp-cpp-test-" + std::to_string(_getpid()), {}};
#else
    char tmpl[] = "/tmp/app-mcp-cpp-ipc-XXXXXX";
    if (!mkdtemp(tmpl)) return std::nullopt;
    return TestEndpoint{std::string("unix:") + tmpl + "/h.sock", tmpl};
#endif
}

}  // namespace

int main(int argc, char** argv) {
    if (argc < 2) {
        std::fprintf(stderr, "用法：%s <fake_host>\n", argv[0]);
        return 2;
    }
    auto ep = make_endpoint();
    if (!ep) {
        std::fprintf(stderr, "FAIL 无法创建临时目录\n");
        return 1;
    }
    std::string cmd = std::string("\"") + argv[1] + "\" --ipc " + ep->endpoint + " --invoke greet --timeout-ms 15000";
    FILE* host = popen(cmd.c_str(), "r");
    if (!host) return 1;

    char buf[8192];
    std::string listening;
    if (std::fgets(buf, sizeof buf, host) && std::strncmp(buf, "LISTENING ", 10) == 0) {
        listening.assign(buf + 10);
        listening.erase(listening.find_last_not_of("\r\n") + 1);
    }
    EXPECT(listening == ep->endpoint);

    int rc = 1;
    if (!listening.empty()) {
        app_mcp::ClientConfig config;
        config.app_id = "ipc-cpp";
        config.app_name = "IPC C++";
        config.host_url = ep->endpoint;
        config.connect_timeout_ms = 5000;

        std::atomic<bool> connected{false};
        app_mcp::ClientCallbacks callbacks;
        callbacks.on_state = [&](const app_mcp::StateInfo& s) {
            if (s.status == AM_STATE_CONNECTED) connected = true;
        };
        app_mcp::Client client(config, callbacks);

        std::mutex cid_mu;
        std::optional<std::string> cid_in_call;
        auto greet = client.register_tool("greet", "问好", [&](app_mcp::Call call) {
            {
                std::lock_guard<std::mutex> lock(cid_mu);
                cid_in_call = client.connection_id();
            }
            call.complete(R"({"greeting":"Hello over IPC!"})");
        });
        client.start();

        bool tools_ok = false, greet_ok = false;
        while (std::fgets(buf, sizeof buf, host)) {
            std::string line(buf);
            std::fprintf(stderr, "[fake_host] %s", buf);
            auto has = [&](const char* s) { return line.find(s) != std::string::npos; };
            if (has("\"type\":\"tools\"") && has("\"greet\"")) tools_ok = true;
            if (has("\"type\":\"invoke\"") && has("Hello over IPC!")) greet_ok = true;
        }
        int status = pclose(host);
        host = nullptr;
#ifndef _WIN32
        status = WIFEXITED(status) ? WEXITSTATUS(status) : -1;
#endif
        EXPECT(status == 0);
        EXPECT(connected);
        EXPECT(tools_ok);
        EXPECT(greet_ok);
        {
            std::lock_guard<std::mutex> lock(cid_mu);
            EXPECT(cid_in_call.has_value() && !cid_in_call->empty());
            EXPECT(cid_in_call.has_value() && cid_in_call->rfind("fake-", 0) == 0);
        }
        rc = g_failed == 0 ? 0 : 1;
    }
    if (host) pclose(host);
#ifndef _WIN32
    // fake_host 退出时删除套接字文件；目录由本测试删除。
    std::remove((ep->dir + "/h.sock").c_str());
    rmdir(ep->dir.c_str());
#endif
    std::fprintf(stderr, rc == 0 ? "ipc_cpp: PASS\n" : "ipc_cpp: FAIL\n");
    return rc;
}
