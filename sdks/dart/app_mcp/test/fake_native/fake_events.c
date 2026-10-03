/*
 * 假 libapp_mcp 的 v20 事件函数（am_client_declare_event / am_client_remove_event / am_client_emit_event）。
 * 与 fake_app_mcp.c 一起编译进同一个假库（见 test/support/fake_native.dart）；单独成文件以免 fake_app_mcp.c 超出行数上限。
 *
 * 只按状态码约定模拟（真实校验由真实库的一致性用例覆盖）：名称为空或含空格 → AM_ERR_INVALID_NAME；schema / 载荷不以 '{' 开头
 * → AM_ERR_INVALID_SCHEMA / AM_ERR_INVALID_JSON；未声明 → AM_ERR_INVALID_NAME。声明表是全局的（不分客户端），
 * fake_events_set_connected 决定 emit 写入的 sent；fake_last_emit 返回最近一次已发送的 "名称|载荷或 -"。
 */
#include "../../../../../bindings/c/include/app_mcp.h"

#include <stdlib.h>
#include <string.h>

#define FAKE_MAX_EVENTS 16

static char *g_events[FAKE_MAX_EVENTS];      /* 已声明的事件名 */
static char *g_event_info[FAKE_MAX_EVENTS];  /* "描述|schema 或 -" */
static int g_events_connected = 0;
static char *g_last_emit = NULL;

static char *fake_events_dup(const char *s) {
    size_t n = strlen(s);
    char *p = malloc(n + 1);
    memcpy(p, s, n + 1);
    return p;
}

static char *fake_events_join(const char *a, const char *b) {
    size_t na = strlen(a), nb = strlen(b);
    char *p = malloc(na + nb + 2);
    memcpy(p, a, na);
    p[na] = '|';
    memcpy(p + na + 1, b, nb + 1);
    return p;
}

static int fake_event_name_ok(const char *name) { return name && *name && !strchr(name, ' '); }

static int fake_event_find(const char *name) {
    for (int i = 0; i < FAKE_MAX_EVENTS; i++) {
        if (g_events[i] && strcmp(g_events[i], name) == 0) return i;
    }
    return -1;
}

AmStatus am_client_declare_event(AmClient *client, const char *name, const char *description,
                                 const char *payload_schema_json) {
    if (!client || !name || !description) return AM_ERR_INVALID_ARGUMENT;
    if (!fake_event_name_ok(name)) return AM_ERR_INVALID_NAME;
    if (payload_schema_json && payload_schema_json[0] != '{') return AM_ERR_INVALID_SCHEMA;
    int i = fake_event_find(name);
    for (int j = 0; i < 0 && j < FAKE_MAX_EVENTS; j++) {
        if (!g_events[j]) i = j;
    }
    if (i < 0) return AM_ERR_INTERNAL;
    free(g_events[i]);
    free(g_event_info[i]);
    g_events[i] = fake_events_dup(name);
    g_event_info[i] = fake_events_join(description, payload_schema_json ? payload_schema_json : "-");
    return AM_OK;
}

AmStatus am_client_remove_event(AmClient *client, const char *name, bool *removed) {
    if (!client || !name) return AM_ERR_INVALID_ARGUMENT;
    int i = fake_event_find(name);
    if (removed) *removed = i >= 0;
    if (i >= 0) {
        free(g_events[i]);
        free(g_event_info[i]);
        g_events[i] = g_event_info[i] = NULL;
    }
    return AM_OK;
}

AmStatus am_client_emit_event(AmClient *client, const char *name, const char *payload_json, bool *sent) {
    if (sent) *sent = false;
    if (!client || !name) return AM_ERR_INVALID_ARGUMENT;
    if (!fake_event_name_ok(name) || fake_event_find(name) < 0) return AM_ERR_INVALID_NAME;
    if (payload_json && payload_json[0] != '{') return AM_ERR_INVALID_JSON;
    if (!g_events_connected) return AM_OK;
    free(g_last_emit);
    g_last_emit = fake_events_join(name, payload_json ? payload_json : "-");
    if (sent) *sent = true;
    return AM_OK;
}

/* 测试驱动：emit 时是否视为已连接。 */
void fake_events_set_connected(int connected) { g_events_connected = connected; }

/* 已声明事件的 "描述|schema 或 -"；未声明返回 NULL。需 am_string_free。 */
char *fake_event_info(const char *name) {
    int i = fake_event_find(name);
    return i < 0 ? NULL : fake_events_dup(g_event_info[i]);
}

/* 最近一次已发送的 "名称|载荷或 -"；没有时 NULL。需 am_string_free。 */
char *fake_last_emit(void) { return g_last_emit ? fake_events_dup(g_last_emit) : NULL; }

/* 清空声明与发送记录（测试间隔离）。 */
void fake_events_reset(void) {
    for (int i = 0; i < FAKE_MAX_EVENTS; i++) {
        free(g_events[i]);
        free(g_event_info[i]);
        g_events[i] = g_event_info[i] = NULL;
    }
    free(g_last_emit);
    g_last_emit = NULL;
    g_events_connected = 0;
}
