/*
 * lifecycle_c.c —— C 接口生命周期集成测试（对真实 fake_host 往返）。
 *
 * 用法：lifecycle_c <fake_host 可执行文件>
 *
 * 流程（spec/lifecycle.md）：
 *   idle 模式（idle_timeout 300ms）连接 → fake_host 调用 reject_tool（am_call_fail_with_details）→
 *   空闲休眠（--await-sleep）→ fake_host 打印唤醒参数 → am_client_handle_wake → 回连且跳过 tools/sync →
 *   调用 greet → 再次休眠。residency = EXIT_ALWAYS，每次休眠后回调 on_idle_exit。
 */
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#ifdef _WIN32
#include <windows.h>
#define popen _popen
#define pclose _pclose
static void sleep_ms(unsigned ms) { Sleep(ms); }
#else
#include <sys/wait.h>
#include <time.h>
static void sleep_ms(unsigned ms) {
    struct timespec ts = {(time_t)(ms / 1000), (long)(ms % 1000) * 1000000L};
    nanosleep(&ts, NULL);
}
#endif

#include "app_mcp.h"

static int g_failed = 0;
#define EXPECT(cond)                                                               \
    do {                                                                           \
        if (!(cond)) {                                                             \
            ++g_failed;                                                            \
            fprintf(stderr, "FAIL %s:%d: %s\n", __FILE__, __LINE__, #cond);        \
        }                                                                          \
    } while (0)

static atomic_int g_idle_exits = 0;
static atomic_int g_held = 0;

static void on_idle_exit(void *ud) {
    (void)ud;
    atomic_fetch_add(&g_idle_exits, 1);
}

static void on_state(void *ud, AmStateStatus status, uint64_t retry, char *reason) {
    (void)ud;
    (void)retry;
    fprintf(stderr, "[lifecycle_c] state=%d\n", (int)status);
    am_string_free(reason);
}

static void reject_tool(void *ud, AmCall *call) {
    (void)ud;
    /* handler 内取得持有再释放（演示 am_call_hold；真实场景在长任务结束后释放）。 */
    AmHold *hold = NULL;
    if (am_call_hold(call, &hold) == AM_OK && hold) atomic_fetch_add(&g_held, 1);
    am_hold_release(hold);
    /* 非法 JSON：不消费，可重试。 */
    if (am_call_fail_with_details(call, "USER_REJECTED", "x", "{bad") != AM_ERR_INVALID_JSON) {
        ++g_failed;
        fprintf(stderr, "FAIL 非法 details 应返回 AM_ERR_INVALID_JSON\n");
    }
    am_call_fail_with_details(call, "USER_REJECTED", "额度不足", "{\"quota\":0,\"hint\":\"upgrade\"}");
}

static void greet(void *ud, AmCall *call) {
    (void)ud;
    am_call_complete(call, "{\"greeting\":\"Hello, World!\"}", NULL, 0);
}

/* 从 {"...","arg":"<value>",...} 中取出 arg。 */
static int extract_arg(const char *line, char *out, size_t cap) {
    const char *p = strstr(line, "\"arg\":\"");
    if (!p) return 0;
    p += 7;
    const char *end = strchr(p, '"');
    if (!end || (size_t)(end - p) >= cap) return 0;
    memcpy(out, p, (size_t)(end - p));
    out[end - p] = '\0';
    return 1;
}

int main(int argc, char **argv) {
    if (argc < 2) {
        fprintf(stderr, "用法：%s <fake_host>\n", argv[0]);
        return 2;
    }
    char cmd[4096];
    /* @compat popen 在 Windows 上经 cmd.exe 执行，不认单引号；命令行里不放带引号的 JSON 参数（greet 不读参数）。 */
    snprintf(cmd, sizeof cmd,
             "\"%s\" --addr 127.0.0.1:0 --invoke reject_tool --await-sleep --wake "
             "--invoke greet --await-sleep --timeout-ms 20000",
             argv[1]);
    FILE *host = popen(cmd, "r");
    if (!host) {
        perror("popen");
        return 1;
    }

    char line[8192];
    char url[256] = {0};
    if (fgets(line, sizeof line, host) && strncmp(line, "LISTENING ", 10) == 0) {
        line[strcspn(line, "\r\n")] = '\0';
        snprintf(url, sizeof url, "ws://%.200s", line + 10);
    }
    if (!url[0]) {
        fprintf(stderr, "FAIL fake_host 没有输出 LISTENING\n");
        pclose(host);
        return 1;
    }

    AmClientConfig config;
    memset(&config, 0, sizeof config);
    config.app_id = "lifecycle-c";
    config.app_name = "Lifecycle C";
    config.host_url = url;

    AmClientCallbacks callbacks;
    memset(&callbacks, 0, sizeof callbacks);
    callbacks.on_state = on_state;

    AmLifecycle lc;
    am_lifecycle_init(&lc);
    EXPECT(lc.idle_timeout_ms == 60000 && lc.wake_kind == AM_WAKE_UNSET);
    lc.mode = AM_LIFECYCLE_IDLE;
    lc.idle_timeout_ms = 300;
    lc.residency = AM_RESIDENCY_EXIT_ALWAYS;
    lc.wake_kind = AM_WAKE_URI;
    lc.wake_target = "lifecycle-c";

    AmClientOptions opts;
    memset(&opts, 0, sizeof opts);
    opts.struct_size = sizeof opts;
    opts.lifecycle = &lc;
    opts.connect_timeout_ms = 2000;
    opts.on_idle_exit = on_idle_exit;

    AmClient *client = NULL;
    AmScope *root = NULL;
    AmTool *t1 = NULL, *t2 = NULL;
    EXPECT(am_client_new_ex(&config, &callbacks, &opts, &client) == AM_OK);
    EXPECT(am_client_root_scope(client, &root) == AM_OK);
    AmToolSpec spec;
    memset(&spec, 0, sizeof spec);
    spec.description = "总是拒绝";
    spec.risk = AM_RISK_READ;
    spec.activation = AM_ACTIVATION_NONE;
    spec.enabled = true;
    spec.name = "reject_tool";
    EXPECT(am_tool_register(root, &spec, reject_tool, NULL, NULL, &t1) == AM_OK);
    spec.name = "greet";
    spec.description = "问好";
    spec.input_schema_json = "{\"type\":\"object\",\"properties\":{\"name\":{\"type\":\"string\"}}}";
    EXPECT(am_tool_register(root, &spec, greet, NULL, NULL, &t2) == AM_OK);
    EXPECT(am_client_start(client) == AM_OK);

    int saw_wake = 0, saw_hello_current = 0, saw_unsynced = 0, sleeps = 0, greet_ok = 0, details_ok = 0;
    while (fgets(line, sizeof line, host)) {
        fprintf(stderr, "[fake_host] %s", line);
        if (strstr(line, "\"type\":\"wake\"")) {
            char arg[512];
            EXPECT(extract_arg(line, arg, sizeof arg));
            /* 必须已经休眠。 */
            AmStateStatus st = AM_STATE_IDLE;
            EXPECT(am_client_state(client, &st, NULL, NULL) == AM_OK);
            EXPECT(st == AM_STATE_DORMANT);
            EXPECT(!am_client_handle_wake(client, "--unrelated"));
            EXPECT(am_client_handle_wake(client, arg));
            saw_wake = 1;
        } else if (strstr(line, "\"type\":\"hello\"")) {
            saw_hello_current = strstr(line, "\"toolsCurrent\":true") && strstr(line, "\"wakeReason\":\"os-activation\"");
        } else if (strstr(line, "\"type\":\"tools\"") && strstr(line, "\"synced\":false")) {
            saw_unsynced = 1;
        } else if (strstr(line, "\"type\":\"sleep\"") && strstr(line, "\"accepted\":true")) {
            ++sleeps;
        } else if (strstr(line, "\"type\":\"invoke\"")) {
            if (strstr(line, "Hello, World!")) greet_ok = 1;
            if (strstr(line, "USER_REJECTED") && strstr(line, "\"hint\":\"upgrade\"")) details_ok = 1;
        }
    }
    int rc = pclose(host);
#ifndef _WIN32
    rc = WIFEXITED(rc) ? WEXITSTATUS(rc) : -1;
#endif
    EXPECT(rc == 0);
    EXPECT(saw_wake);
    EXPECT(saw_hello_current);
    EXPECT(saw_unsynced);
    EXPECT(sleeps == 2);
    EXPECT(greet_ok);
    EXPECT(details_ok);
    EXPECT(atomic_load(&g_held) == 1);

    /* 第二次休眠后客户端回到 dormant，两次休眠各回调一次 on_idle_exit。 */
    AmStateStatus st = AM_STATE_IDLE;
    for (int i = 0; i < 50; ++i) {
        am_client_state(client, &st, NULL, NULL);
        if (st == AM_STATE_DORMANT && atomic_load(&g_idle_exits) >= 2) break;
        sleep_ms(50);
    }
    EXPECT(st == AM_STATE_DORMANT);
    EXPECT(atomic_load(&g_idle_exits) == 2);

    char *hash = am_client_tools_hash(client);
    EXPECT(hash && strlen(hash) == 16);
    am_string_free(hash);

    am_tool_free(t1);
    am_tool_free(t2);
    am_scope_free(root);
    am_client_free(client);
    if (g_failed) {
        fprintf(stderr, "lifecycle_c: %d 项失败\n", g_failed);
        return 1;
    }
    fprintf(stderr, "lifecycle_c: PASS\n");
    return 0;
}
