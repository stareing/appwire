/*
 * 假 libapp_mcp 的结构体布局探针：返回 app_mcp.h 中结构体的大小与字段偏移，供 Dart 测试核对 FFI 布局。
 * 与 fake_app_mcp.c 一起编译进同一个假库（见 test/support/fake_native.dart）。
 */
#include "../../../../../bindings/c/include/app_mcp.h"

#include <stddef.h>

/* 结构体布局，供 Dart 测试核对。 */
size_t fake_sizeof(int which) {
    switch (which) {
    case 0: return sizeof(AmClientConfig);
    case 1: return sizeof(AmClientCallbacks);
    case 2: return sizeof(AmToolSpec);
    case 3: return sizeof(AmResourceSpec);
    case 4: return offsetof(AmClientConfig, overview_locale);
    case 5: return offsetof(AmToolSpec, enabled);
    case 6: return sizeof(AmLifecycle);
    case 7: return sizeof(AmClientOptions);
    case 8: return offsetof(AmLifecycle, wake_target);
    case 9: return offsetof(AmClientOptions, on_idle_exit);
    case 10: return offsetof(AmClientOptions, heartbeat);
    case 11: return offsetof(AmClientOptions, legacy_timers);
    case 12: return offsetof(AmClientOptions, merge_window_ms);
    case 13: return offsetof(AmClientOptions, sleep_on_background);
    case 14: return sizeof(AmResourceOptions);
    case 15: return offsetof(AmResourceOptions, realtime);
    case 16: return sizeof(AmToolOptions);
    case 17: return sizeof(AmCallResult);
    case 18: return offsetof(AmCallResult, status);
    case 19: return offsetof(AmCallResult, annotations_json);
    case 20: return offsetof(AmClientOptions, call_dedup_ttl_ms);
    case 21: return offsetof(AmClientOptions, call_dedup_max_entries);
    case 22: return offsetof(AmResourceOptions, annotations_json);
    case 23: return offsetof(AmClientOptions, register_name);
    case 24: return offsetof(AmClientOptions, name_instance);
    case 25: return offsetof(AmClientOptions, max_queued_calls);
    case 26: return offsetof(AmToolOptions, exclusive);
    case 27: return offsetof(AmToolOptions, implements);
    case 28: return offsetof(AmToolOptions, implements_len);
    default: return 0;
    }
}
