// hello.cpp —— 用 C++ 封装注册 greet({name}) 工具与 app.info 资源，连接 Host，Ctrl+C 退出。
//
// 用法：hello_cpp [ws://127.0.0.1:7717]（或环境变量 APP_MCP_HOST_URL）

#include <atomic>
#include <chrono>
#include <csignal>
#include <cstdlib>
#include <iostream>
#include <string>
#include <thread>

#include "app_mcp.hpp"

namespace {

std::atomic<bool> g_quit{false};

extern "C" void on_signal(int) { g_quit = true; }

/// 极简的取值：从 {"name":"..."} 中取出 name（不处理转义；真实项目请使用 JSON 库，如 nlohmann/json）。
std::string extract_name(const std::string& json) {
    auto key = json.find("\"name\"");
    if (key == std::string::npos) return {};
    auto start = json.find('"', json.find(':', key + 6));
    if (start == std::string::npos) return {};
    auto end = json.find('"', start + 1);
    if (end == std::string::npos) return {};
    return json.substr(start + 1, end - start - 1);
}

}  // namespace

int main(int argc, char** argv) {
    std::signal(SIGINT, on_signal);
    std::signal(SIGTERM, on_signal);

    app_mcp::ClientConfig config;
    config.app_id = "hello-cpp";
    config.app_name = "Hello C++";
    if (argc > 1) {
        config.host_url = argv[1];
    } else if (const char* env = std::getenv("APP_MCP_HOST_URL")) {
        config.host_url = env;
    }

    app_mcp::ClientCallbacks callbacks;
    callbacks.on_state = [](const app_mcp::StateInfo& s) {
        std::cerr << "[hello_cpp] state=" << s.status << " retry_in_ms=" << s.retry_in_ms << " " << s.reason << "\n";
    };
    callbacks.on_paired = [](const std::string& token) { std::cerr << "[hello_cpp] paired, token=" << token << "\n"; };

    try {
        std::cerr << "[hello_cpp] app_mcp " << app_mcp::version() << "\n";
        app_mcp::Client client(config, callbacks);

        app_mcp::ToolOptions opts;
        opts.input_schema_json =
            R"({"type":"object","properties":{"name":{"type":"string"}},"required":["name"]})";
        opts.risk = AM_RISK_READ;
        auto greet = client.register_tool(
            "greet", "向指定的人问好",
            [](app_mcp::Call call) {
                std::string name = extract_name(call.arguments_json());
                if (name.empty()) throw app_mcp::ToolCallError("INVALID_INPUT", "缺少 name 参数");
                // 本例直接在分发线程上完成。需要 UI 线程时，把 call 移动过去再 complete。
                call.complete(R"({"greeting":)" + app_mcp::json_quote("Hello, " + name + "!") + "}");
            },
            opts);

        auto info = client.register_resource("app.info", "App 基本信息", [](app_mcp::Read read) {
            read.complete(R"({"app":"hello_cpp","lang":"c++"})");
        });

        client.start();
        std::cerr << "[hello_cpp] 已启动，Ctrl+C 退出\n";
        while (!g_quit) std::this_thread::sleep_for(std::chrono::milliseconds(100));
    } catch (const app_mcp::Error& e) {
        std::cerr << "[hello_cpp] 错误 " << e.status() << ": " << e.what() << "\n";
        return 1;
    }
    std::cerr << "[hello_cpp] 退出\n";
    return 0;
}
