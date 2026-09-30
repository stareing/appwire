/*
 * hello.c —— 用 C 接口注册 greet({name}) 工具与 app.info 资源，连接 Host，Ctrl+C 退出。
 *
 * 用法：hello_c [ws://127.0.0.1:7717]
 * 也可用环境变量 APP_MCP_HOST_URL 指定 Host 地址。
 */
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#ifdef _WIN32
#include <windows.h>
static void sleep_ms(unsigned ms) { Sleep(ms); }
#else
#include <time.h>
static void sleep_ms(unsigned ms) {
    struct timespec ts = {(time_t)(ms / 1000), (long)(ms % 1000) * 1000000L};
    nanosleep(&ts, NULL);
}
#endif

#include "app_mcp.h"

static volatile sig_atomic_t g_quit = 0;
static void on_signal(int sig) {
    (void)sig;
    g_quit = 1;
}

static const char *state_name(AmStateStatus s) {
    static const char *names[] = {"idle",      "connecting", "handshaking", "pending_pairing", "connected",
                                  "backoff",   "rejected",   "stopped",     "dormant",         "waking"};
    return (s >= 0 && s <= AM_STATE_WAKING) ? names[s] : "?";
}

/* 以下三个回调中的字符串归回调方所有，用完后以 am_string_free 释放（am_string_free(NULL) 无操作）。 */
static void on_state(void *ud, AmStateStatus status, uint64_t retry_in_ms, char *reason) {
    (void)ud;
    fprintf(stderr, "[hello_c] state=%s retry_in_ms=%llu reason=%s\n", state_name(status),
            (unsigned long long)retry_in_ms, reason ? reason : "");
    am_string_free(reason);
}

static void on_paired(void *ud, char *token) {
    (void)ud;
    fprintf(stderr, "[hello_c] paired, token=%s\n", token);
    am_string_free(token);
}

static void on_log(void *ud, AmLogLevel level, char *message) {
    (void)ud;
    fprintf(stderr, "[hello_c] log(%d) %s\n", (int)level, message);
    am_string_free(message);
}

/*
 * 极简的参数解析：从 {"name":"..."} 中取出 name（不处理转义；真实项目请使用 JSON 库）。
 * 成功返回 malloc 的字符串。
 */
static char *extract_name(const char *json) {
    const char *key = strstr(json, "\"name\"");
    if (!key) return NULL;
    const char *colon = strchr(key + 6, ':');
    if (!colon) return NULL;
    const char *start = strchr(colon, '"');
    if (!start) return NULL;
    start++;
    const char *end = strchr(start, '"');
    if (!end) return NULL;
    size_t len = (size_t)(end - start);
    char *out = (char *)malloc(len + 1);
    if (!out) return NULL;
    memcpy(out, start, len);
    out[len] = '\0';
    return out;
}

static void greet(void *ud, AmCall *call) {
    (void)ud;
    /* 本例直接在分发线程上完成；需要 UI 线程时，把 call 投递过去再完成。 */
    char *name = extract_name(am_call_arguments_json(call));
    if (!name) {
        am_call_fail(call, "INVALID_INPUT", "缺少 name 参数");
        return;
    }
    size_t cap = strlen(name) + 64;
    char *data = (char *)malloc(cap);
    if (!data) {
        free(name);
        am_call_fail(call, "HANDLER_ERROR", "内存不足");
        return;
    }
    snprintf(data, cap, "{\"greeting\":\"Hello, %s!\"}", name);
    AmStatus s = am_call_complete(call, data, NULL, 0);
    if (s == AM_ERR_INVALID_JSON) {
        am_call_fail(call, "HANDLER_ERROR", am_last_error_message());
    }
    free(data);
    free(name);
}

static void read_info(void *ud, AmRead *read) {
    (void)ud;
    am_read_complete(read, "{\"app\":\"hello_c\",\"lang\":\"c\"}");
}

#define CHECK(expr)                                                                    \
    do {                                                                               \
        AmStatus s_ = (expr);                                                          \
        if (s_ != AM_OK) {                                                             \
            fprintf(stderr, "%s 失败：%d %s\n", #expr, (int)s_, am_last_error_message()); \
            goto cleanup;                                                              \
        }                                                                              \
    } while (0)

int main(int argc, char **argv) {
    const char *url = argc > 1 ? argv[1] : getenv("APP_MCP_HOST_URL");
    AmClient *client = NULL;
    AmScope *root = NULL;
    AmTool *tool = NULL;
    AmResource *res = NULL;
    int rc = 1;

    signal(SIGINT, on_signal);
    signal(SIGTERM, on_signal);

    AmClientConfig config;
    memset(&config, 0, sizeof config);
    config.app_id = "hello-c";
    config.app_name = "Hello C";
    config.host_url = url; /* NULL：默认 ws://127.0.0.1:7717 */
    config.client_kind = AM_CLIENT_NATIVE;

    AmClientCallbacks callbacks;
    memset(&callbacks, 0, sizeof callbacks);
    callbacks.on_state = on_state;
    callbacks.on_paired = on_paired;
    callbacks.on_log = on_log;

    fprintf(stderr, "[hello_c] app_mcp %s\n", am_version());
    CHECK(am_client_new(&config, &callbacks, &client));
    CHECK(am_client_root_scope(client, &root));

    AmToolSpec greet_spec;
    memset(&greet_spec, 0, sizeof greet_spec);
    greet_spec.name = "greet";
    greet_spec.description = "向指定的人问好";
    greet_spec.input_schema_json =
        "{\"type\":\"object\",\"properties\":{\"name\":{\"type\":\"string\"}},\"required\":[\"name\"]}";
    greet_spec.risk = AM_RISK_READ;
    greet_spec.activation = AM_ACTIVATION_NONE;
    greet_spec.enabled = true;
    CHECK(am_tool_register(root, &greet_spec, greet, NULL, NULL, &tool));

    AmResourceSpec info_spec = {"app.info", "App 基本信息", NULL};
    CHECK(am_resource_register(root, &info_spec, read_info, NULL, NULL, &res));

    CHECK(am_client_start(client));
    fprintf(stderr, "[hello_c] 已启动，Ctrl+C 退出\n");
    while (!g_quit) sleep_ms(100);
    rc = 0;

cleanup:
    am_tool_free(tool);
    am_resource_free(res);
    am_scope_free(root);
    am_client_free(client);
    fprintf(stderr, "[hello_c] 退出\n");
    return rc;
}
