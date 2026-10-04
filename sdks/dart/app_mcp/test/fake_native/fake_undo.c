/*
 * 假 libapp_mcp 的 v24 撤销记录：AmToolOptions 末尾的 undoable 与 AmCallResult 末尾的 undo_tool / undo_arguments_json /
 * undo_label（均按 struct_size 读取）。与 fake_app_mcp.c 一起编译进同一个假库（见 test/support/fake_native.dart）；单独成文件
 * 以免 fake_app_mcp.c 超出行数上限。
 *
 * 只记录不校验（格式由真实库的核心校验，见一致性用例 result-undo）。记录表只在调用方线程（Dart 主 isolate）读写，不加锁：
 * fake_undoable_of 返回 "1" / "0"；fake_call_undo 返回 "<tool 或 ->|<arguments 或 ->|<label 或 ->"、无撤销信息为 "-"。
 */
#include "../../../../../bindings/c/include/app_mcp.h"

#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define FAKE_MAX_UNDO_TOOLS 64
#define FAKE_MAX_UNDO_CALLS 256

static char *g_undo_tools[FAKE_MAX_UNDO_TOOLS];
static int g_undoable[FAKE_MAX_UNDO_TOOLS];
static char *g_undo_calls[FAKE_MAX_UNDO_CALLS];

static char *fake_undo_dup(const char *s) {
    size_t n = strlen(s);
    char *p = malloc(n + 1);
    memcpy(p, s, n + 1);
    return p;
}

/* fake_app_mcp.c 的 apply_tool_options 调用。旧调用方的 struct_size 不含 undoable 时视为未声明。 */
void fake_undo_record_tool(const char *tool, const AmToolOptions *o) {
    int v24 = o && o->struct_size >= offsetof(AmToolOptions, undoable) + sizeof o->undoable;
    int slot = -1;
    for (int i = 0; i < FAKE_MAX_UNDO_TOOLS; i++) {
        if (g_undo_tools[i] && strcmp(g_undo_tools[i], tool) == 0) { slot = i; break; }
        if (!g_undo_tools[i] && slot < 0) slot = i;
    }
    if (slot < 0) return; /* 表满：测试不会注册这么多 */
    if (!g_undo_tools[slot]) g_undo_tools[slot] = fake_undo_dup(tool);
    g_undoable[slot] = v24 && o->undoable;
}

/* v24：工具当前的 undoable 声明（需 am_string_free）；未记录过时返回 NULL。 */
char *fake_undoable_of(const char *tool) {
    for (int i = 0; i < FAKE_MAX_UNDO_TOOLS; i++) {
        if (g_undo_tools[i] && strcmp(g_undo_tools[i], tool) == 0) return fake_undo_dup(g_undoable[i] ? "1" : "0");
    }
    return NULL;
}

/* fake_app_mcp.c 的 am_call_complete_ex 调用（index 为调用序号）。旧调用方的 struct_size 不含 undo_label 时视为无撤销信息。 */
void fake_undo_record_result(int index, const AmCallResult *r) {
    if (index < 0 || index >= FAKE_MAX_UNDO_CALLS) return;
    int v24 = r->struct_size >= offsetof(AmCallResult, undo_label) + sizeof r->undo_label;
    char value[1024] = "-";
    if (v24 && (r->undo_tool || r->undo_arguments_json || r->undo_label)) {
        snprintf(value, sizeof value, "%s|%s|%s", r->undo_tool ? r->undo_tool : "-",
                 r->undo_arguments_json ? r->undo_arguments_json : "-", r->undo_label ? r->undo_label : "-");
    }
    free(g_undo_calls[index]);
    g_undo_calls[index] = fake_undo_dup(value);
}

/* v24：调用以 am_call_complete_ex 完成时的撤销信息（需 am_string_free）；未经 am_call_complete_ex 完成时返回 NULL。 */
char *fake_call_undo(int index) {
    if (index < 0 || index >= FAKE_MAX_UNDO_CALLS || !g_undo_calls[index]) return NULL;
    return fake_undo_dup(g_undo_calls[index]);
}
