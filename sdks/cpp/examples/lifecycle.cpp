// lifecycle.cpp —— 生命周期示例：on-demand 模式 + 唤醒入口 + 空闲退出。
//
// 用法：lifecycle_cpp [ws://127.0.0.1:7717/app] [激活参数...]
//
// - on-demand：启动时不连接；Host 需要时通过唤醒描述（这里是 URI scheme "lifecycle-cpp"）拉起 /
//   叫醒本进程，参数形如 `lifecycle-cpp://app-mcp/wake?token=...` 或 `app-mcp-wake:<token>`。
// - 冷启动：main 的参数交给 handle_wake；已运行时由单实例机制（命名管道、D-Bus 等）把第二实例的
//   参数转交过来——本示例用标准输入模拟：每行一条激活参数。
// - residency = exit-when-idle：由唤醒冷启动的进程在任务完成、休眠后收到 on_idle_exit，于是退出。
#include <atomic>
#include <chrono>
#include <cstdio>
#include <iostream>
#include <string>
#include <thread>

#include "app_mcp.hpp"

int main(int argc, char** argv) {
    std::atomic<bool> quit{false};

    app_mcp::ClientConfig config;
    config.app_id = "lifecycle-cpp";
    config.app_name = "Lifecycle C++";
    if (argc > 1 && std::string(argv[1]).rfind("ws", 0) == 0) config.host_url = argv[1];
    config.lifecycle.mode = AM_LIFECYCLE_ON_DEMAND;
    config.lifecycle.grace_ms = 3000;
    config.lifecycle.residency = AM_RESIDENCY_EXIT_WHEN_IDLE;
    config.lifecycle.wake = app_mcp::WakeDescriptor{AM_WAKE_URI, std::string("lifecycle-cpp"), true};

    app_mcp::ClientCallbacks callbacks;
    callbacks.on_state = [](const app_mcp::StateInfo& s) { std::fprintf(stderr, "[lifecycle] state=%d\n", s.status); };
    // 在分发线程上：只设置标志，由主线程退出（不要在回调里析构 Client）。
    callbacks.on_idle_exit = [&] { quit = true; };

    try {
        app_mcp::Client client(config, callbacks);
        auto export_tool = client.register_tool("export", "导出报表（长任务）", [&](app_mcp::Call call) {
            // 长任务：完成调用后仍阻止休眠，直到后台工作结束。
            auto hold = std::make_shared<app_mcp::HoldGuard>(call.hold());
            call.complete(R"({"started":true})");
            std::thread([hold] { std::this_thread::sleep_for(std::chrono::seconds(2)); }).detach();
        });
        auto quota = client.register_tool("quota", "查询额度", [](app_mcp::Call) {
            throw app_mcp::ToolCallError("USER_REJECTED", "额度不足", std::string(R"({"remaining":0})"));
        });

        // 冷启动唤醒：on-demand 模式下 handle_wake 会发起连接。
        if (client.handle_wake(argc, argv)) std::fprintf(stderr, "[lifecycle] 由唤醒启动\n");
        client.start();

        // 模拟单实例转发：每行一条激活参数；"sleep" / "wake" 手动控制。
        std::thread input([&] {
            std::string line;
            while (!quit && std::getline(std::cin, line)) {
                if (line == "sleep") client.sleep();
                else if (line == "wake") client.wake();
                else if (!client.handle_wake(line)) std::fprintf(stderr, "[lifecycle] 忽略：%s\n", line.c_str());
            }
        });
        input.detach();
        while (!quit) std::this_thread::sleep_for(std::chrono::milliseconds(100));
        std::fprintf(stderr, "[lifecycle] 空闲退出\n");
    } catch (const app_mcp::Error& e) {
        std::fprintf(stderr, "[lifecycle] 错误 %d：%s\n", e.status(), e.what());
        return 1;
    }
    return 0;
}
