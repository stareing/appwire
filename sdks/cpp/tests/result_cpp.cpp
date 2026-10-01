// result_cpp.cpp —— C++ 封装的工具声明与结构化调用结果对接 fake_host 的集成测试（app_mcp.h v9）。
//
// 用法：result_cpp_test <fake_host 可执行文件>
// 检查：带注解 + outputSchema 注册后 Host 收到的 ToolInfo（fake_host --tool-info）；
//       以 pending + state_resource + summary + 内容注解完成后 Host 收到的结果；普通返回值不变（回归）。
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>

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

}  // namespace

int main(int argc, char** argv) {
    if (argc < 2) {
        std::fprintf(stderr, "用法：%s <fake_host>\n", argv[0]);
        return 2;
    }
    std::string cmd = std::string("\"") + argv[1] +
                      "\" --addr 127.0.0.1:0 --tool-info --invoke order.submit --invoke plain --timeout-ms 15000";
    FILE* host = popen(cmd.c_str(), "r");
    if (!host) return 1;

    char buf[16384];
    std::string addr;
    if (std::fgets(buf, sizeof buf, host) && std::strncmp(buf, "LISTENING ", 10) == 0) {
        addr.assign(buf + 10);
        addr.erase(addr.find_last_not_of("\r\n") + 1);
    }
    EXPECT(!addr.empty());
    if (addr.empty()) {
        pclose(host);
        return 1;
    }

    app_mcp::ClientConfig config;
    config.app_id = "result-cpp";
    config.app_name = "Result C++";
    config.host_url = "ws://" + addr;
    app_mcp::Client client(config);

    app_mcp::ToolOptions options;
    options.annotations = app_mcp::ToolAnnotations{};
    options.annotations->idempotent_hint = false;
    options.annotations->open_world_hint = true;
    options.output_schema_json = R"({"type":"object","properties":{"orderId":{"type":"string"}}})";
    auto submit = client.register_tool("order.submit", "下单", [](app_mcp::Call call) {
        app_mcp::CallResult result;
        result.data_json = R"({"orderId":"o1"})";
        result.status = AM_RESULT_PENDING;
        result.state_resource = "order.state";
        result.summary = "已提交，等待用户在 App 内付款";
        result.annotations = app_mcp::ContentAnnotations{};
        result.annotations->priority = 0.5;
        call.complete(result);
    }, options);
    app_mcp::ToolOptions plain_options;
    plain_options.risk = AM_RISK_READ;
    auto plain = client.register_tool("plain", "普通", [](app_mcp::Call call) {
        call.progress(1, 2.0, std::string("处理中"));
        call.progress(2);
        call.complete(R"({"ok":true})");
        call.progress(3);  // 已完成：无副作用
    }, plain_options);
    client.start();

    bool info_ok = false, plain_info_ok = false, result_ok = false, plain_ok = false;
    int progress_lines = 0;
    bool progress_ok = false;
    while (std::fgets(buf, sizeof buf, host)) {
        std::string line(buf);
        std::fprintf(stderr, "[fake_host] %s", buf);
        auto has = [&](const char* s) { return line.find(s) != std::string::npos; };
        if (has("\"type\":\"tools\"")) {
            info_ok = has(R"("order.submit":{"annotations":{"idempotentHint":false,"openWorldHint":true},)"
                          R"("outputSchema":{"properties":{"orderId":{"type":"string"}},"type":"object"},"risk":"write"})");
            plain_info_ok = has(R"("plain":{"risk":"read"})");
        }
        if (has("\"type\":\"invoke\"") && has("\"order.submit\"")) {
            result_ok = has(R"("annotations":{"priority":0.5})") && has(R"("data":{"orderId":"o1"})") &&
                        has(R"("stateResource":"order.state")") && has(R"("status":"pending")") &&
                        has("已提交，等待用户在 App 内付款");
        }
        if (has("\"type\":\"progress\"")) {
            ++progress_lines;
            if (has(R"("message":"处理中","progress":1.0,"total":2.0)")) progress_ok = true;
        }
        if (has("\"type\":\"invoke\"") && has("\"plain\"")) {
            plain_ok = has(R"("result":{"data":{"ok":true}})");
        }
    }
    int status = pclose(host);
#ifndef _WIN32
    status = WIFEXITED(status) ? WEXITSTATUS(status) : -1;
#endif
    EXPECT(status == 0);
    EXPECT(info_ok);
    EXPECT(plain_info_ok);
    EXPECT(result_ok);
    EXPECT(plain_ok);
    EXPECT(progress_ok);
    EXPECT(progress_lines == 2);
    int rc = g_failed == 0 ? 0 : 1;
    std::fprintf(stderr, rc == 0 ? "result_cpp: PASS\n" : "result_cpp: FAIL\n");
    return rc;
}
