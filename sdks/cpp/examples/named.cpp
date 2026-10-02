// named.cpp —— 按名寻址示例（spec/naming.md，app_mcp.h v17）：在系统名字服务登记、不主动连接 Hub，
// 由 Hub 拨号时接受通道；通道关闭后若本进程由激活启动（命令行带 --app-mcp-activation）就退出，进程交还系统。
//
//   named_cpp                                                        # 用户直接运行：登记名字，常驻直到 Ctrl-C
//   app-mcp-host app install --app-id named-cpp --exec <本程序绝对路径>   # 登记：Linux 写 D-Bus 激活文件，
//                                                                    # Windows 写 %LOCALAPPDATA%\app-mcp\apps\named-cpp.json
//   app-mcp-host serve --name-service                                # Hub 按名发现、调用时拨号（未运行则由系统激活）
//
// 环境变量：APP_MCP_APP_ID（默认 named-cpp）；APP_MCP_NAME_INSTANCE（可选，登记实例名）；
// APP_MCP_EVENT_LOG（可选，追加 "start <pid>" / "exit <pid>" 行，测试用来核对激活次数与退出）。
// 工具：echo（原样返回参数）、pid（返回进程号）。
#include <atomic>
#include <cstdio>
#include <cstdlib>
#include <fstream>
#include <future>
#include <string>

#include "app_mcp.hpp"

#ifdef _WIN32
#include <process.h>
#define APP_MCP_GETPID _getpid
#else
#include <unistd.h>
#define APP_MCP_GETPID getpid
#endif

namespace {

std::string env_or(const char* name, const char* fallback) {
    const char* v = std::getenv(name);
    return v && *v ? std::string(v) : std::string(fallback);
}

void note(const char* event) {
    const char* path = std::getenv("APP_MCP_EVENT_LOG");
    if (!path || !*path) return;
    std::ofstream f(path, std::ios::app);
    f << event << ' ' << static_cast<long>(APP_MCP_GETPID()) << '\n';
}

}  // namespace

int main() {
    note("start");
    std::promise<void> idle_exit;
    std::atomic<bool> exiting{false};

    app_mcp::ClientConfig config;
    config.app_id = env_or("APP_MCP_APP_ID", "named-cpp");
    config.app_name = "按名寻址示例（C++）";
    config.lifecycle.mode = AM_LIFECYCLE_ON_DEMAND;
    config.lifecycle.residency = AM_RESIDENCY_EXIT_WHEN_IDLE;
    config.register_name = true;
    std::string instance = env_or("APP_MCP_NAME_INSTANCE", "");
    if (!instance.empty()) config.name_instance = instance;

    app_mcp::ClientCallbacks callbacks;
    callbacks.on_log = [](app_mcp::LogLevel, const std::string& m) { std::fprintf(stderr, "[named_cpp] %s\n", m.c_str()); };
    // 在分发线程上：只设置标志，由主线程退出（不要在回调里析构 Client）。
    callbacks.on_idle_exit = [&] {
        if (!exiting.exchange(true)) idle_exit.set_value();
    };

    try {
        app_mcp::Client client(config, callbacks);
        auto echo = client.register_tool("echo", "原样返回参数", [](app_mcp::Call call) {
            std::string args = call.arguments_json();
            call.complete("{\"echo\":" + args + "}");
        });
        auto pid = client.register_tool("pid", "返回进程号", [](app_mcp::Call call) {
            call.complete("{\"pid\":" + std::to_string(static_cast<long>(APP_MCP_GETPID())) + "}");
        });
        client.start();
        // 由激活启动：通道关闭后收到 on_idle_exit 即退出；用户直接运行时不会收到，一直等待。
        idle_exit.get_future().wait();
    } catch (const app_mcp::Error& e) {
        std::fprintf(stderr, "[named_cpp] 错误 %d：%s\n", e.status(), e.what());
        return 2;
    }
    note("exit");
    return 0;
}
