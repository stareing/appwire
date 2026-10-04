/*
 * 假 libapp_mcp 的 v22 结果缓存声明记录（AmToolOptions / AmResourceOptions 末尾的 cache_ttl_ms / cache_scope，按
 * struct_size 读取）。与 fake_app_mcp.c 一起编译进同一个假库（见 test/support/fake_native.dart）；单独成文件以免
 * fake_app_mcp.c 超出行数上限。
 *
 * 只记录不校验（ttl 范围的校验由真实库与 Hub 集成测试覆盖）。记录表按 "tool:<名>" / "resource:<名>"（全局，不分客户端），
 * 只在调用方线程（Dart 主 isolate）读写，不加锁；fake_cache_of 返回 "<ttl>|<scope 数值>"、未声明为 "-"。
 */
#include "../../../../../bindings/c/include/app_mcp.h"

#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define FAKE_MAX_CACHE_ENTRIES 64

static char *g_cache_keys[FAKE_MAX_CACHE_ENTRIES];   /* "tool:<名>" / "resource:<名>" */
static char *g_cache_values[FAKE_MAX_CACHE_ENTRIES]; /* "<ttl>|<scope>" 或 "-" */

static char *fake_cache_dup(const char *s) {
    size_t n = strlen(s);
    char *p = malloc(n + 1);
    memcpy(p, s, n + 1);
    return p;
}

static void fake_cache_put(const char *kind, const char *name, int declared, uint64_t ttl_ms, int scope) {
    char key[160];
    snprintf(key, sizeof key, "%s:%s", kind, name);
    char value[64] = "-";
    if (declared && ttl_ms > 0) snprintf(value, sizeof value, "%llu|%d", (unsigned long long)ttl_ms, scope);
    int slot = -1;
    for (int i = 0; i < FAKE_MAX_CACHE_ENTRIES; i++) {
        if (g_cache_keys[i] && strcmp(g_cache_keys[i], key) == 0) { slot = i; break; }
        if (!g_cache_keys[i] && slot < 0) slot = i;
    }
    if (slot < 0) return; /* 表满：测试不会注册这么多 */
    if (!g_cache_keys[slot]) g_cache_keys[slot] = fake_cache_dup(key);
    free(g_cache_values[slot]);
    g_cache_values[slot] = fake_cache_dup(value);
}

/* fake_app_mcp.c 的 apply_tool_options 调用。旧调用方的 struct_size 不含 cache_scope 时视为未声明。 */
void fake_cache_record_tool(const char *tool, const AmToolOptions *o) {
    int v22 = o && o->struct_size >= offsetof(AmToolOptions, cache_scope) + sizeof o->cache_scope;
    fake_cache_put("tool", tool, v22, v22 ? o->cache_ttl_ms : 0, v22 ? o->cache_scope : 0);
}

/* fake_app_mcp.c 的 am_resource_register_ex 成功后调用。 */
void fake_cache_record_resource(const char *resource, const AmResourceOptions *o) {
    int v22 = o && o->struct_size >= offsetof(AmResourceOptions, cache_scope) + sizeof o->cache_scope;
    fake_cache_put("resource", resource, v22, v22 ? o->cache_ttl_ms : 0, v22 ? o->cache_scope : 0);
}

/* v22：key（"tool:<名>" / "resource:<名>"）当前的缓存声明（需 am_string_free）；未记录过时返回 NULL。 */
char *fake_cache_of(const char *key) {
    for (int i = 0; i < FAKE_MAX_CACHE_ENTRIES; i++) {
        if (g_cache_keys[i] && strcmp(g_cache_keys[i], key) == 0) return fake_cache_dup(g_cache_values[i]);
    }
    return NULL;
}
