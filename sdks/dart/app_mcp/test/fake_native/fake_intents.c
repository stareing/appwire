/*
 * 假 libapp_mcp 的 v21 工具 implements 记录（AmToolOptions 末尾的 implements / implements_len，按 struct_size 读取）。
 * 与 fake_app_mcp.c 一起编译进同一个假库（见 test/support/fake_native.dart）；单独成文件以免 fake_app_mcp.c 超出行数上限。
 *
 * 只记录不校验（动词格式的校验由真实库与 Hub 集成测试覆盖）。记录表按工具名（全局，不分客户端），
 * 只在调用方线程（Dart 主 isolate）读写，不加锁；fake_tool_implements 返回 "a,b"、未声明为 "-"。
 */
#include "../../../../../bindings/c/include/app_mcp.h"

#include <stddef.h>
#include <stdlib.h>
#include <string.h>

#define FAKE_MAX_IMPLEMENTS_TOOLS 64

static char *g_impl_tools[FAKE_MAX_IMPLEMENTS_TOOLS];  /* 工具名 */
static char *g_impl_values[FAKE_MAX_IMPLEMENTS_TOOLS]; /* "a,b" 或 "-" */

static char *fake_intents_dup(const char *s, size_t n) {
    char *p = malloc(n + 1);
    memcpy(p, s, n);
    p[n] = '\0';
    return p;
}

/* 旧调用方的 struct_size 不含 implements_len 时视为未声明。 */
static char *fake_intents_join(const AmToolOptions *o) {
    int v21 = o && o->struct_size >= offsetof(AmToolOptions, implements_len) + sizeof o->implements_len;
    if (!v21 || o->implements_len == 0 || !o->implements) return fake_intents_dup("-", 1);
    size_t total = 0;
    for (size_t i = 0; i < o->implements_len; i++) total += strlen(o->implements[i]) + 1;
    char *p = malloc(total);
    size_t at = 0;
    for (size_t i = 0; i < o->implements_len; i++) {
        size_t n = strlen(o->implements[i]);
        memcpy(p + at, o->implements[i], n);
        at += n;
        p[at++] = i + 1 < o->implements_len ? ',' : '\0';
    }
    return p;
}

/* fake_app_mcp.c 的 apply_tool_options 调用：替换该工具的记录。 */
void fake_intents_record(const char *tool, const AmToolOptions *o) {
    int slot = -1;
    for (int i = 0; i < FAKE_MAX_IMPLEMENTS_TOOLS; i++) {
        if (g_impl_tools[i] && strcmp(g_impl_tools[i], tool) == 0) { slot = i; break; }
        if (!g_impl_tools[i] && slot < 0) slot = i;
    }
    if (slot < 0) return; /* 表满：测试不会注册这么多工具 */
    if (!g_impl_tools[slot]) g_impl_tools[slot] = fake_intents_dup(tool, strlen(tool));
    free(g_impl_values[slot]);
    g_impl_values[slot] = fake_intents_join(o);
}

/* v21：工具当前的 implements（"a,b"，未声明为 "-"；需 am_string_free）；未记录过时返回 NULL。 */
char *fake_tool_implements(const char *tool) {
    for (int i = 0; i < FAKE_MAX_IMPLEMENTS_TOOLS; i++) {
        if (g_impl_tools[i] && strcmp(g_impl_tools[i], tool) == 0) {
            return fake_intents_dup(g_impl_values[i], strlen(g_impl_values[i]));
        }
    }
    return NULL;
}
