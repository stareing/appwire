/*
 * 假 libapp_mcp 的 v23 弃用声明记录（AmToolOptions 末尾的 deprecated_message / deprecated_replacement / deprecated_until，
 * 按 struct_size 读取）。与 fake_app_mcp.c 一起编译进同一个假库（见 test/support/fake_native.dart）；单独成文件以免
 * fake_app_mcp.c 超出行数上限。
 *
 * 只记录不校验（格式校验由真实库与 Hub 集成测试覆盖）。记录表按工具名（全局，不分客户端），只在调用方线程（Dart 主
 * isolate）读写，不加锁；fake_deprecated_of 返回 "<message>|<replacement 或 ->|<until 或 ->"、未声明为 "-"。
 */
#include "../../../../../bindings/c/include/app_mcp.h"

#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define FAKE_MAX_DEPRECATED_ENTRIES 64

static char *g_dep_tools[FAKE_MAX_DEPRECATED_ENTRIES];
static char *g_dep_values[FAKE_MAX_DEPRECATED_ENTRIES];

static char *fake_dep_dup(const char *s) {
    size_t n = strlen(s);
    char *p = malloc(n + 1);
    memcpy(p, s, n + 1);
    return p;
}

/* fake_app_mcp.c 的 apply_tool_options 调用。旧调用方的 struct_size 不含 deprecated_until 时视为未声明。 */
void fake_deprecated_record_tool(const char *tool, const AmToolOptions *o) {
    int v23 = o && o->struct_size >= offsetof(AmToolOptions, deprecated_until) + sizeof o->deprecated_until;
    char value[1024] = "-";
    if (v23 && (o->deprecated_message || o->deprecated_replacement || o->deprecated_until)) {
        snprintf(value, sizeof value, "%s|%s|%s", o->deprecated_message ? o->deprecated_message : "",
                 o->deprecated_replacement ? o->deprecated_replacement : "-",
                 o->deprecated_until ? o->deprecated_until : "-");
    }
    int slot = -1;
    for (int i = 0; i < FAKE_MAX_DEPRECATED_ENTRIES; i++) {
        if (g_dep_tools[i] && strcmp(g_dep_tools[i], tool) == 0) { slot = i; break; }
        if (!g_dep_tools[i] && slot < 0) slot = i;
    }
    if (slot < 0) return; /* 表满：测试不会注册这么多 */
    if (!g_dep_tools[slot]) g_dep_tools[slot] = fake_dep_dup(tool);
    free(g_dep_values[slot]);
    g_dep_values[slot] = fake_dep_dup(value);
}

/* v23：工具当前的弃用声明（需 am_string_free）；未记录过时返回 NULL。 */
char *fake_deprecated_of(const char *tool) {
    for (int i = 0; i < FAKE_MAX_DEPRECATED_ENTRIES; i++) {
        if (g_dep_tools[i] && strcmp(g_dep_tools[i], tool) == 0) return fake_dep_dup(g_dep_values[i]);
    }
    return NULL;
}
