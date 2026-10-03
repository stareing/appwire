/*
 * 测试用的假 libapp_mcp：按 app_mcp.h 实现全部函数，但不连接 Host。
 *
 * 所有回调都在新建的线程（pthread / Win32 线程）上触发（模拟库的分发线程）。按 API 版本 2，状态 / 配对 / 日志
 * 回调中的字符串归接收方所有；假库记录这些字符串，am_string_free 时注销，
 * fake_owned_outstanding 返回尚未释放的数量，用来检验 Dart 封装会释放它们。
 * 额外导出 fake_* 函数供测试驱动调用、取消与状态变化。
 *
 * v3（生命周期）：am_client_new_ex 记录生命周期配置（fake_lifecycle），handle_wake 识别
 * "app-mcp-wake:" 前缀，wake / sleep / connect_now 发出状态变化，hold 计数（fake_hold_count），
 * fake_idle_exit 在其他线程触发 on_idle_exit。
 * v7 / v8（4e 功耗）：fake_lifecycle 末尾追加 heartbeat、host_absent_retries、legacy_timers、merge_window_ms、
 * sleep_on_background；am_resource_register_ex 记录 realtime（fake_resource_realtime）。
 * v9：am_tool_register_ex / am_tool_update_ex 记录注解与 outputSchema（fake_tool_options；outputSchema 为 "{" 时
 * 返回 AM_ERR_INVALID_SCHEMA）；am_call_complete_ex 的结果另带 status / stateResource / summary / annotations
 * （annotations_json 为 "{bad" 时返回 AM_ERR_INVALID_JSON）。
 * v13：am_client_new_ex 记录调用去重（fake_call_dedup："<ttl_ms>|<max_entries>"）；am_resource_register_ex 记录
 * 资源内容标注（fake_resource_annotations；annotations_json 为 "{bad" 时返回 AM_ERR_INVALID_JSON）。
 * v17：am_client_new_ex 记录按名寻址（fake_name_service："<register_name 0/1>|<name_instance 或 ->"）。
 * v18：am_client_new_ex 记录 max_queued_calls（fake_max_queued_calls）；工具记录 concurrency / exclusive（fake_tool_schedule）。
 * 结构体布局探针 fake_sizeof 在 fake_layout.c。
 *
 * 编译：cc -shared -fPIC -o libfake_app_mcp.so fake_app_mcp.c fake_layout.c -lpthread
 * Windows（MSVC）：cl /c /utf-8 编译后按 dumpbin /symbols 中的外部函数生成 .def 再 link /DLL
 * （与 cc 默认导出全部非 static 函数一致，见 test/support/fake_native.dart）。
 */
#include "../../../../../bindings/c/include/app_mcp.h"

#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* ------------------------------------------------------------------ 平台差异：线程、锁 */
#ifdef _WIN32
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <process.h>
#define FAKE_THREAD_LOCAL __declspec(thread)
static SRWLOCK g_lock = SRWLOCK_INIT;
static void lock_global(void) { AcquireSRWLockExclusive(&g_lock); }
static void unlock_global(void) { ReleaseSRWLockExclusive(&g_lock); }
#else
#include <pthread.h>
#define FAKE_THREAD_LOCAL __thread
static pthread_mutex_t g_lock = PTHREAD_MUTEX_INITIALIZER;
static void lock_global(void) { pthread_mutex_lock(&g_lock); }
static void unlock_global(void) { pthread_mutex_unlock(&g_lock); }
#endif

static int g_free_count = 0;
static FAKE_THREAD_LOCAL char g_last_error[256];

static char *dup_str(const char *s) {
    if (!s) return NULL;
    size_t n = strlen(s);
    char *p = malloc(n + 1);
    memcpy(p, s, n + 1);
    return p;
}

static void set_error(const char *msg) { snprintf(g_last_error, sizeof g_last_error, "%s", msg); }

/* ------------------------------------------------------------------ 数据结构 */

typedef struct ScopeRec {
    struct ScopeRec *parent;
    int disposed;
} ScopeRec;

typedef struct ToolRec {
    char *name;
    char *description;
    char *schema;
    int risk, activation, enabled;
    ScopeRec *scope;
    int disposed;
    AmToolFn handler;
    void *user_data;
    AmFreeFn free_user_data;
    int freed;
    char *annotations;   /* v9 */
    char *output_schema; /* v9 */
    char *page;          /* v14 */
    int surface;         /* v14 */
    char *background_tool; /* v15 */
    uint32_t concurrency; char *exclusive; /* v18 */
} ToolRec;

typedef struct ResRec {
    char *name;
    ScopeRec *scope;
    int disposed;
    AmReadFn reader;
    void *user_data;
    AmFreeFn free_user_data;
    int freed;
    int realtime;
    char *annotations; /* v13 */
} ResRec;

#define MAX_ITEMS 256

struct AmClient {
    AmClientCallbacks cb;
    char *token;
    char *instance_id;
    int status;
    uint64_t retry;
    char *reason;
    char *code; /* v6：当前状态的错误码 */
    ScopeRec root;
    ToolRec *tools[MAX_ITEMS];
    int n_tools;
    ResRec *res[MAX_ITEMS];
    int n_res;
    int stopped;
    AmIdleExitFn on_idle_exit;
    bool busy; int busy_policy; /* v19：am_client_set_busy / am_client_set_busy_policy 的当前值 */
};
struct AmScope { AmClient *client; ScopeRec *rec; };
struct AmTool { AmClient *client; ToolRec *rec; };
struct AmResource { AmClient *client; ResRec *rec; };

struct AmCall {
    int index;
    char *id, *tool_name, *args;
    char *idempotency_key; /* v16：NULL = 无幂等键 */
    int cancelled;
    AmCancelFn on_cancel;
    void *cancel_ud;
    AmFreeFn cancel_free;
};
struct AmRead { int index; char *name; };

static char *g_results[MAX_ITEMS];
static AmCall *g_calls[MAX_ITEMS];
static int g_n_calls = 0;
static char *g_overview = NULL;    /* 最近一次配置的总览，"summary|body|locale" */
static AmClient *g_client = NULL; /* 最近创建的客户端，供 fake_* 使用 */

static int scope_alive(ScopeRec *s) {
    for (; s; s = s->parent)
        if (s->disposed) return 0;
    return 1;
}

/* ------------------------------------------------------------------ 线程工具 */

typedef void (*job_fn)(void *);
typedef struct { job_fn f; void *arg; } Job;
static void job_run(void *p) {
    Job j = *(Job *)p;
    free(p);
    j.f(j.arg);
}
#ifdef _WIN32
static unsigned __stdcall job_main(void *p) {
    job_run(p);
    return 0;
}
static void run_on_thread(job_fn f, void *arg) {
    Job *j = malloc(sizeof *j);
    j->f = f;
    j->arg = arg;
    uintptr_t t = _beginthreadex(NULL, 0, job_main, j, 0, NULL);
    if (t) CloseHandle((HANDLE)t);
}
#else
static void *job_main(void *p) {
    job_run(p);
    return NULL;
}
static void run_on_thread(job_fn f, void *arg) {
    Job *j = malloc(sizeof *j);
    j->f = f;
    j->arg = arg;
    pthread_t t;
    pthread_create(&t, NULL, job_main, j);
    pthread_detach(t);
}
#endif

typedef struct { AmFreeFn f; void *ud; } FreeJob;
static void free_job(void *p) {
    FreeJob *j = p;
    j->f(j->ud);
    lock_global();
    g_free_count++;
    unlock_global();
    free(j);
}
/* 在另一个线程上调用 free_user_data（头文件允许任意线程）。 */
static void release_user_data(AmFreeFn f, void *ud) {
    if (!f) return;
    FreeJob *j = malloc(sizeof *j);
    j->f = f;
    j->ud = ud;
    run_on_thread(free_job, j);
}

/* ------------------------------------------------------------------ 通用 */

const char *am_version(void) { return "0.0.0-fake"; }
const char *am_last_error_message(void) { return g_last_error; }
/* 交给回调方的字符串（v2：归接收方所有，须 am_string_free）。 */
static char *g_owned[4096];
static int g_owned_total = 0;
static char *give_owned(const char *s) {
    char *p = dup_str(s);
    if (!p) return NULL;
    lock_global();
    for (int i = 0; i < (int)(sizeof g_owned / sizeof g_owned[0]); i++) {
        if (!g_owned[i]) { g_owned[i] = p; break; }
    }
    g_owned_total++;
    unlock_global();
    return p;
}
void am_string_free(char *s) {
    if (!s) return;
    lock_global();
    for (int i = 0; i < (int)(sizeof g_owned / sizeof g_owned[0]); i++) {
        if (g_owned[i] == s) { g_owned[i] = NULL; break; }
    }
    unlock_global();
    free(s);
}

/* ------------------------------------------------------------------ 客户端 */

static char *g_lifecycle = NULL; /* 最近一次配置的生命周期，见 am_client_new_ex */
static char *g_call_dedup = NULL; /* v13：最近一次配置的调用去重，见 am_client_new_ex */
static char *g_name_service = NULL; /* v17：最近一次配置的按名寻址，见 am_client_new_ex */
static int32_t g_max_queued = 0;     /* v18：最近一次 am_client_new_ex 的 max_queued_calls */
static AmStatus client_new(const AmClientConfig *config, const AmClientCallbacks *callbacks, AmClient **out) {
    if (!config || !out || !config->app_id || !config->app_name) {
        set_error("缺少必填参数");
        return AM_ERR_INVALID_ARGUMENT;
    }
    if (config->app_id[0] == '\0') {
        set_error("appId 不能为空");
        return AM_ERR_INVALID_CONFIG;
    }
    AmClient *c = calloc(1, sizeof *c);
    if (callbacks) c->cb = *callbacks;
    c->token = dup_str(config->token);
    c->instance_id = dup_str(config->instance_id ? config->instance_id : "fake-instance");
    free(g_overview);
    if (config->overview_summary) {
        size_t cap = 16 + strlen(config->overview_summary) + (config->overview_body ? strlen(config->overview_body) : 0) +
                     (config->overview_locale ? strlen(config->overview_locale) : 0);
        g_overview = malloc(cap);
        snprintf(g_overview, cap, "%s|%s|%s", config->overview_summary,
                 config->overview_body ? config->overview_body : "-", config->overview_locale ? config->overview_locale : "-");
    } else {
        g_overview = NULL;
    }
    *out = c;
    g_client = c;
    return AM_OK;
}

AmStatus am_client_new(const AmClientConfig *config, const AmClientCallbacks *callbacks, AmClient **out) {
    return client_new(config, callbacks, out);
}

void am_lifecycle_init(AmLifecycle *l) {
    if (!l) return;
    memset(l, 0, sizeof *l);
    l->mode = AM_LIFECYCLE_PERSISTENT;
    l->idle_timeout_ms = 60000;
    l->hidden_idle_timeout_ms = 15000;
    l->grace_ms = 10000;
    l->residency = AM_RESIDENCY_KEEP;
    l->wake_kind = AM_WAKE_UNSET;
}

AmStatus am_client_new_ex(const AmClientConfig *config, const AmClientCallbacks *callbacks,
                          const AmClientOptions *options, AmClient **out) {
    if (options && options->struct_size != sizeof(AmClientOptions)) {
        set_error("struct_size 不匹配");
        return AM_ERR_INVALID_ARGUMENT;
    }
    AmStatus st = client_new(config, callbacks, out);
    if (st != AM_OK) return st;
    free(g_lifecycle);
    g_lifecycle = NULL;
    free(g_call_dedup);
    g_call_dedup = NULL;
    free(g_name_service);
    g_name_service = NULL;
    if (options) {
        char naming[64];
        snprintf(naming, sizeof naming, "%d|%s", options->register_name ? 1 : 0,
                 options->name_instance ? options->name_instance : "-");
        g_name_service = dup_str(naming);
        g_max_queued = options->max_queued_calls;
        char dedup[64];
        snprintf(dedup, sizeof dedup, "%lld|%d", (long long)options->call_dedup_ttl_ms,
                 (int)options->call_dedup_max_entries);
        g_call_dedup = dup_str(dedup);
        (*out)->on_idle_exit = options->on_idle_exit;
        const AmLifecycle *l = options->lifecycle;
        char buf[512];
        char power[128];
        snprintf(power, sizeof power, "%d|%d|%d|%lld|%d", (int)options->heartbeat, (int)options->host_absent_retries,
                 options->legacy_timers ? 1 : 0, (long long)options->merge_window_ms,
                 options->sleep_on_background ? 1 : 0);
        if (l) {
            snprintf(buf, sizeof buf, "%d|%llu|%llu|%llu|%d|%d|%s|%d|%u|%s", (int)l->mode,
                     (unsigned long long)l->idle_timeout_ms, (unsigned long long)l->hidden_idle_timeout_ms,
                     (unsigned long long)l->grace_ms, (int)l->residency, (int)l->wake_kind,
                     l->wake_target ? l->wake_target : "-", l->wake_background ? 1 : 0, options->connect_timeout_ms, power);
        } else {
            snprintf(buf, sizeof buf, "-|%u|%s", options->connect_timeout_ms, power);
        }
        g_lifecycle = dup_str(buf);
    }
    return AM_OK;
}

void am_client_free(AmClient *c) {
    if (!c) return;
    for (int i = 0; i < c->n_tools; i++) {
        ToolRec *t = c->tools[i];
        if (!t->freed) { t->freed = 1; release_user_data(t->free_user_data, t->user_data); }
    }
    for (int i = 0; i < c->n_res; i++) {
        ResRec *r = c->res[i];
        if (!r->freed) { r->freed = 1; release_user_data(r->free_user_data, r->user_data); }
    }
    if (c->cb.free_user_data) release_user_data(c->cb.free_user_data, c->cb.user_data);
    /* 故意泄漏 c，避免在途线程访问已释放内存（测试进程很快退出）。 */
    c->stopped = 1;
}

typedef struct { AmClient *c; int status; uint64_t retry; char *reason; } StateJob;
static void state_job(void *p) {
    StateJob *j = p;
    if (j->c->cb.on_state) j->c->cb.on_state(j->c->cb.user_data, (AmStateStatus)j->status, j->retry, j->reason);
    else am_string_free(j->reason);
    free(j);
}
static void emit_state_code(AmClient *c, int status, uint64_t retry, const char *reason, const char *code) {
    lock_global();
    c->status = status;
    c->retry = retry;
    free(c->reason);
    c->reason = dup_str(reason);
    free(c->code);
    c->code = dup_str(code);
    unlock_global();
    StateJob *j = malloc(sizeof *j);
    j->c = c; j->status = status; j->retry = retry; j->reason = give_owned(reason);
    run_on_thread(state_job, j);
}
static void emit_state(AmClient *c, int status, uint64_t retry, const char *reason) {
    emit_state_code(c, status, retry, reason, NULL);
}

AmStatus am_client_start(AmClient *c) {
    if (!c) return AM_ERR_INVALID_ARGUMENT;
    if (c->stopped) { set_error("客户端已停止"); return AM_ERR_STOPPED; }
    emit_state(c, AM_STATE_CONNECTED, 0, NULL);
    return AM_OK;
}
AmStatus am_client_stop(AmClient *c) {
    if (!c) return AM_ERR_INVALID_ARGUMENT;
    c->stopped = 1;
    return AM_OK;
}
static int g_visibility = -1, g_focused = -1;
AmStatus am_client_set_visibility(AmClient *c, AmVisibility v, bool focused) {
    if (!c) return AM_ERR_INVALID_ARGUMENT;
    g_visibility = v;
    g_focused = focused;
    return AM_OK;
}
AmStatus am_client_state(const AmClient *c, AmStateStatus *status, uint64_t *retry, char **reason) {
    if (!c || !status) return AM_ERR_INVALID_ARGUMENT;
    lock_global();
    *status = (AmStateStatus)c->status;
    if (retry) *retry = c->retry;
    if (reason) {
        /* v6：REJECTED / HOST_MISMATCH 总有原因，BACKOFF 有原因时也给出。 */
        int always = c->status == AM_STATE_REJECTED || c->status == AM_STATE_HOST_MISMATCH;
        *reason = always ? dup_str(c->reason ? c->reason : "")
                         : (c->status == AM_STATE_BACKOFF ? dup_str(c->reason) : NULL);
    }
    unlock_global();
    return AM_OK;
}
/* v6：与真实库一致，只在 BACKOFF / REJECTED / HOST_MISMATCH 时可能非 NULL。 */
AmStatus am_client_state_code(const AmClient *c, char **code) {
    if (!c || !code) return AM_ERR_INVALID_ARGUMENT;
    lock_global();
    int has = c->status == AM_STATE_BACKOFF || c->status == AM_STATE_REJECTED || c->status == AM_STATE_HOST_MISMATCH;
    *code = has ? dup_str(c->code) : NULL;
    unlock_global();
    return AM_OK;
}
/* v6：CONNECTED 时返回固定的假连接 ID。 */
AmStatus am_client_connection_id(const AmClient *c, char **id) {
    if (!c || !id) return AM_ERR_INVALID_ARGUMENT;
    lock_global();
    *id = c->status == AM_STATE_CONNECTED ? dup_str("fake-cid-1") : NULL;
    unlock_global();
    return AM_OK;
}
char *am_client_instance_id(const AmClient *c) { return dup_str(c->instance_id); }
char *am_client_token(const AmClient *c) {
    lock_global();
    char *t = dup_str(c->token);
    unlock_global();
    return t;
}
AmStatus am_client_root_scope(AmClient *c, AmScope **out) {
    if (!c || !out) return AM_ERR_INVALID_ARGUMENT;
    AmScope *s = malloc(sizeof *s);
    s->client = c;
    s->rec = &c->root;
    *out = s;
    return AM_OK;
}

/* ------------------------------------------------------------------ 生命周期（v3） */

struct AmHold { int released; };
static int g_holds = 0;       /* 未释放的持有数 */
static char *g_last_sleep = NULL;

bool am_client_handle_wake(AmClient *c, const char *args) {
    if (!c || !args) { set_error("空指针"); return false; }
    if (strncmp(args, "app-mcp-wake:", 13) != 0 || args[13] == '\0') return false;
    emit_state(c, AM_STATE_WAKING, 0, NULL);
    return true;
}
AmStatus am_client_wake_with_reason(AmClient *c, AmWakeReason reason, bool *started) {
    if (!c) return AM_ERR_INVALID_ARGUMENT;
    if (reason < 0 || reason > 3) { set_error("非法的 wake reason"); return AM_ERR_INVALID_ARGUMENT; }
    int dormant = c->status == AM_STATE_DORMANT;
    if (dormant) emit_state(c, AM_STATE_WAKING, 0, NULL);
    if (started) *started = dormant;
    return AM_OK;
}
AmStatus am_client_wake(AmClient *c, bool *started) { return am_client_wake_with_reason(c, AM_WAKE_REASON_APP, started); }
AmStatus am_client_connect_now(AmClient *c, bool *started) {
    if (!c) return AM_ERR_INVALID_ARGUMENT;
    emit_state(c, AM_STATE_CONNECTING, 0, NULL);
    if (started) *started = true;
    return AM_OK;
}
AmStatus am_client_sleep_with_reason(AmClient *c, AmSleepReason reason, bool *changed) {
    if (!c) return AM_ERR_INVALID_ARGUMENT;
    char buf[16];
    snprintf(buf, sizeof buf, "%d", (int)reason);
    lock_global();
    free(g_last_sleep);
    g_last_sleep = dup_str(buf);
    unlock_global();
    int was = c->status != AM_STATE_DORMANT;
    emit_state(c, AM_STATE_DORMANT, 0, NULL);
    if (changed) *changed = was;
    return AM_OK;
}
AmStatus am_client_sleep(AmClient *c, bool *changed) { return am_client_sleep_with_reason(c, AM_SLEEP_REASON_APP, changed); }
static AmHold *new_hold(void) {
    AmHold *h = calloc(1, sizeof *h);
    lock_global();
    g_holds++;
    unlock_global();
    return h;
}
AmStatus am_client_hold(AmClient *c, AmHold **out) {
    if (!c || !out) return AM_ERR_INVALID_ARGUMENT;
    *out = new_hold();
    return AM_OK;
}
void am_hold_release(AmHold *h) {
    if (!h) return;
    lock_global();
    g_holds--;
    unlock_global();
    free(h);
}
char *am_client_tools_hash(const AmClient *c) {
    if (!c) return NULL;
    char buf[32];
    snprintf(buf, sizeof buf, "%016x", c->n_tools);
    return dup_str(buf);
}
char *am_parse_wake_token(const char *args) {
    if (!args || strncmp(args, "app-mcp-wake:", 13) != 0 || args[13] == '\0') return NULL;
    return dup_str(args + 13);
}

typedef struct { AmClient *c; } IdleExitJob;
static void idle_exit_job(void *p) {
    IdleExitJob *j = p;
    if (j->c->on_idle_exit) j->c->on_idle_exit(j->c->cb.user_data);
    free(j);
}
/* 在其他线程上触发 on_idle_exit。 */
void fake_idle_exit(void) {
    IdleExitJob *j = malloc(sizeof *j);
    j->c = g_client;
    run_on_thread(idle_exit_job, j);
}
int fake_hold_count(void) {
    lock_global();
    int n = g_holds;
    unlock_global();
    return n;
}
/* 最近一次生命周期配置（需 am_string_free）；没有 options 时返回 NULL。 */
char *fake_lifecycle(void) { return dup_str(g_lifecycle); }
/* v13：最近一次 am_client_new_ex 的调用去重，"<ttl_ms>|<max_entries>"（需 am_string_free）；没有 options 时 NULL。 */
char *fake_call_dedup(void) { return dup_str(g_call_dedup); }
/* v17：最近一次 am_client_new_ex 的按名寻址，"<0/1>|<name_instance 或 ->"（需 am_string_free）；没有 options 时 NULL。 */
char *fake_name_service(void) { return dup_str(g_name_service); }
/* v18：最近一次 am_client_new_ex 的 max_queued_calls（原样）。 */
int32_t fake_max_queued_calls(void) { return g_max_queued; }
/* v19：用户正在操作。只记录状态（拒绝 / 排队的行为由真实库的一致性用例覆盖）；非法策略 → AM_ERR_INVALID_ARGUMENT。 */
AmStatus am_client_set_busy(AmClient *c, bool busy) { if (!c) return AM_ERR_INVALID_ARGUMENT; c->busy = busy; return AM_OK; }
AmStatus am_client_is_busy(const AmClient *c, bool *out) { if (!c || !out) return AM_ERR_INVALID_ARGUMENT; *out = c->busy; return AM_OK; }
AmStatus am_client_set_busy_policy(AmClient *c, AmBusyPolicy p) {
    if (!c || (p != AM_BUSY_REJECT && p != AM_BUSY_QUEUE)) { set_error("非法的 busy 策略"); return AM_ERR_INVALID_ARGUMENT; }
    c->busy_policy = (int)p;
    return AM_OK;
}
int fake_busy_policy(void) { return g_client ? g_client->busy_policy : -1; }
int fake_busy(void) { return g_client ? (int)g_client->busy : -1; }
/* 最近一次 sleep 的原因（需 am_string_free）。 */
char *fake_last_sleep(void) {
    lock_global();
    char *s = dup_str(g_last_sleep);
    unlock_global();
    return s;
}

/* ------------------------------------------------------------------ Scope */

AmStatus am_scope_create(AmScope *parent, const char *name, AmScope **out) {
    if (!parent || !name || !out) return AM_ERR_INVALID_ARGUMENT;
    if (parent->client->stopped) { set_error("客户端已停止"); return AM_ERR_STOPPED; }
    ScopeRec *r = calloc(1, sizeof *r);
    r->parent = parent->rec;
    AmScope *s = malloc(sizeof *s);
    s->client = parent->client;
    s->rec = r;
    *out = s;
    return AM_OK;
}
AmStatus am_scope_dispose(AmScope *s) {
    if (!s) return AM_ERR_INVALID_ARGUMENT;
    AmClient *c = s->client;
    if (s->rec == &c->root) {
        /* 根作用域：注销全部，但根本身仍可用。 */
        for (int i = 0; i < c->n_tools; i++) c->tools[i]->disposed = 1;
        for (int i = 0; i < c->n_res; i++) c->res[i]->disposed = 1;
    } else {
        s->rec->disposed = 1;
    }
    for (int i = 0; i < c->n_tools; i++) {
        ToolRec *t = c->tools[i];
        if ((t->disposed || !scope_alive(t->scope)) && !t->freed) {
            t->disposed = 1; t->freed = 1;
            release_user_data(t->free_user_data, t->user_data);
        }
    }
    for (int i = 0; i < c->n_res; i++) {
        ResRec *r = c->res[i];
        if ((r->disposed || !scope_alive(r->scope)) && !r->freed) {
            r->disposed = 1; r->freed = 1;
            release_user_data(r->free_user_data, r->user_data);
        }
    }
    return AM_OK;
}
void am_scope_free(AmScope *s) { free(s); }

/* ------------------------------------------------------------------ 工具 */

static int valid_name(const char *n) {
    size_t len = strlen(n);
    if (len == 0 || len > 64) return 0;
    for (const char *p = n; *p; p++) {
        char ch = *p;
        if (!((ch >= 'a' && ch <= 'z') || (ch >= 'A' && ch <= 'Z') || (ch >= '0' && ch <= '9') ||
              ch == '_' || ch == '.' || ch == '-'))
            return 0;
    }
    return 1;
}

static ToolRec *find_tool(AmClient *c, const char *name) {
    for (int i = 0; i < c->n_tools; i++) {
        ToolRec *t = c->tools[i];
        if (!t->disposed && scope_alive(t->scope) && strcmp(t->name, name) == 0) return t;
    }
    return NULL;
}

static AmStatus apply_spec(ToolRec *t, const AmToolSpec *spec) {
    if (!spec->description) { set_error("缺少描述"); return AM_ERR_INVALID_ARGUMENT; }
    if (spec->input_schema_json && !strstr(spec->input_schema_json, "\"type\":\"object\"")) {
        set_error("inputSchema 的 type 必须为 object");
        return AM_ERR_INVALID_SCHEMA;
    }
    free(t->description);
    free(t->schema);
    t->description = dup_str(spec->description);
    t->schema = dup_str(spec->input_schema_json);
    t->risk = spec->risk;
    t->activation = spec->activation;
    t->enabled = spec->enabled;
    return AM_OK;
}

AmStatus am_tool_register(AmScope *scope, const AmToolSpec *spec, AmToolFn handler, void *user_data,
                          AmFreeFn free_user_data, AmTool **out) {
    if (!scope || !spec || !spec->name || !handler || !out) { set_error("空指针"); return AM_ERR_INVALID_ARGUMENT; }
    AmClient *c = scope->client;
    if (c->stopped) { set_error("客户端已停止"); return AM_ERR_STOPPED; }
    if (!valid_name(spec->name)) { set_error("非法名称"); return AM_ERR_INVALID_NAME; }
    if (find_tool(c, spec->name)) { set_error("重名"); return AM_ERR_DUPLICATE_NAME; }
    ToolRec *t = calloc(1, sizeof *t);
    AmStatus st = apply_spec(t, spec);
    if (st != AM_OK) { free(t); return st; }
    t->name = dup_str(spec->name);
    t->scope = scope->rec;
    t->handler = handler;
    t->user_data = user_data;
    t->free_user_data = free_user_data;
    c->tools[c->n_tools++] = t;
    AmTool *h = malloc(sizeof *h);
    h->client = c;
    h->rec = t;
    *out = h;
    return AM_OK;
}
AmStatus am_tool_update(AmTool *tool, const AmToolSpec *spec) {
    if (!tool || !spec) return AM_ERR_INVALID_ARGUMENT;
    if (tool->rec->disposed) { set_error("已注销"); return AM_ERR_DISPOSED; }
    return apply_spec(tool->rec, spec);
}
/* v9：options 为 NULL 时清除；outputSchema 为 "{" 时视为非法 JSON。 */
static AmStatus check_tool_options(const AmToolOptions *o) {
    if (o && o->output_schema_json && strcmp(o->output_schema_json, "{") == 0) {
        set_error("outputSchema 不是合法 JSON");
        return AM_ERR_INVALID_SCHEMA;
    }
    return AM_OK;
}
static void apply_tool_options(ToolRec *t, const AmToolOptions *o) {
    free(t->annotations);
    free(t->output_schema);
    free(t->page);
    free(t->background_tool);
    free(t->exclusive);
    t->annotations = o ? dup_str(o->annotations_json) : NULL;
    t->output_schema = o ? dup_str(o->output_schema_json) : NULL;
    /* v14：按 struct_size 读取（旧调用方不含 page / surface）。 */
    int v14 = o && o->struct_size >= offsetof(AmToolOptions, surface) + sizeof o->surface;
    t->page = v14 ? dup_str(o->page) : NULL;
    t->surface = v14 ? o->surface : AM_SURFACE_APP;
    /* v15：按 struct_size 读取（旧调用方不含 background_tool）。 */
    int v15 = o && o->struct_size >= offsetof(AmToolOptions, background_tool) + sizeof o->background_tool;
    t->background_tool = v15 ? dup_str(o->background_tool) : NULL;
    int v18 = o && o->struct_size >= offsetof(AmToolOptions, exclusive) + sizeof o->exclusive;
    t->concurrency = v18 ? o->concurrency : 0;
    t->exclusive = v18 ? dup_str(o->exclusive) : NULL;
}
AmStatus am_tool_register_ex(AmScope *scope, const AmToolSpec *spec, const AmToolOptions *options,
                             AmToolFn handler, void *user_data, AmFreeFn free_user_data, AmTool **out) {
    AmStatus st = check_tool_options(options);
    if (st != AM_OK) return st;
    st = am_tool_register(scope, spec, handler, user_data, free_user_data, out);
    if (st == AM_OK) apply_tool_options((*out)->rec, options);
    return st;
}
AmStatus am_tool_update_ex(AmTool *tool, const AmToolSpec *spec, const AmToolOptions *options) {
    AmStatus st = check_tool_options(options);
    if (st != AM_OK) return st;
    st = am_tool_update(tool, spec);
    if (st == AM_OK) apply_tool_options(tool->rec, options);
    return st;
}
AmStatus am_tool_set_enabled(AmTool *tool, bool enabled) {
    if (!tool) return AM_ERR_INVALID_ARGUMENT;
    if (tool->rec->disposed) { set_error("已注销"); return AM_ERR_DISPOSED; }
    tool->rec->enabled = enabled;
    return AM_OK;
}
AmStatus am_tool_dispose(AmTool *tool) {
    if (!tool) return AM_ERR_INVALID_ARGUMENT;
    ToolRec *t = tool->rec;
    t->disposed = 1;
    if (!t->freed) { t->freed = 1; release_user_data(t->free_user_data, t->user_data); }
    return AM_OK;
}
void am_tool_free(AmTool *tool) { free(tool); }

/* ------------------------------------------------------------------ 资源 */

AmStatus am_resource_register_ex(AmScope *scope, const AmResourceSpec *spec, const AmResourceOptions *options,
                                 AmReadFn reader, void *user_data, AmFreeFn free_user_data, AmResource **out) {
    if (options && options->struct_size != sizeof(AmResourceOptions)) {
        set_error("struct_size 不匹配");
        return AM_ERR_INVALID_ARGUMENT;
    }
    if (options && options->annotations_json && strcmp(options->annotations_json, "{bad") == 0) {
        set_error("annotations_json 不合法");
        return AM_ERR_INVALID_JSON;
    }
    AmStatus st = am_resource_register(scope, spec, reader, user_data, free_user_data, out);
    if (st == AM_OK && options) {
        (*out)->rec->realtime = options->realtime ? 1 : 0;
        if (options->annotations_json) (*out)->rec->annotations = dup_str(options->annotations_json);
    }
    return st;
}

AmStatus am_resource_register(AmScope *scope, const AmResourceSpec *spec, AmReadFn reader, void *user_data,
                              AmFreeFn free_user_data, AmResource **out) {
    if (!scope || !spec || !spec->name || !reader || !out) return AM_ERR_INVALID_ARGUMENT;
    AmClient *c = scope->client;
    if (!valid_name(spec->name)) { set_error("非法名称"); return AM_ERR_INVALID_NAME; }
    ResRec *r = calloc(1, sizeof *r);
    r->name = dup_str(spec->name);
    r->scope = scope->rec;
    r->reader = reader;
    r->user_data = user_data;
    r->free_user_data = free_user_data;
    c->res[c->n_res++] = r;
    AmResource *h = malloc(sizeof *h);
    h->client = c;
    h->rec = r;
    *out = h;
    return AM_OK;
}
static int g_notify_count = 0;
AmStatus am_resource_notify_changed(AmResource *r) {
    if (!r) return AM_ERR_INVALID_ARGUMENT;
    g_notify_count++;
    return AM_OK;
}
AmStatus am_resource_dispose(AmResource *res) {
    if (!res) return AM_ERR_INVALID_ARGUMENT;
    ResRec *r = res->rec;
    r->disposed = 1;
    if (!r->freed) { r->freed = 1; release_user_data(r->free_user_data, r->user_data); }
    return AM_OK;
}
void am_resource_free(AmResource *r) { free(r); }

/* ------------------------------------------------------------------ 调用 */

const char *am_call_id(const AmCall *call) { return call->id; }
const char *am_call_tool_name(const AmCall *call) { return call->tool_name; }
const char *am_call_arguments_json(const AmCall *call) { return call->args; }
const char *am_call_idempotency_key(const AmCall *call) { return call ? call->idempotency_key : NULL; }
bool am_call_is_cancelled(const AmCall *call) {
    lock_global();
    int c = call->cancelled;
    unlock_global();
    return c;
}
AmStatus am_call_set_cancel_callback(AmCall *call, AmCancelFn on_cancel, void *ud, AmFreeFn free_ud) {
    if (!call || !on_cancel) return AM_ERR_INVALID_ARGUMENT;
    lock_global();
    call->on_cancel = on_cancel;
    call->cancel_ud = ud;
    call->cancel_free = free_ud;
    int cancelled = call->cancelled;
    unlock_global();
    if (cancelled) on_cancel(ud, AM_CANCEL_REQUESTED);
    return AM_OK;
}

static void consume(AmCall *call, char *result) {
    lock_global();
    g_results[call->index] = result;
    g_calls[call->index] = NULL;
    AmFreeFn f = call->cancel_free;
    void *ud = call->cancel_ud;
    unlock_global();
    release_user_data(f, ud);
    free(call->id);
    free(call->tool_name);
    memset(call->args, 0, strlen(call->args));
    free(call->args);
    free(call->idempotency_key);
    free(call);
}

AmStatus am_call_complete(AmCall *call, const char *data_json, const char *const *hints, size_t n) {
    if (!call) return AM_ERR_INVALID_ARGUMENT;
    if (data_json && strcmp(data_json, "{bad") == 0) { set_error("非法 JSON"); return AM_ERR_INVALID_JSON; }
    int cancelled = am_call_is_cancelled(call);
    size_t cap = 64 + (data_json ? strlen(data_json) : 4);
    for (size_t i = 0; i < n; i++) cap += strlen(hints[i]) + 4;
    char *buf = malloc(cap);
    int off = snprintf(buf, cap, "{\"ok\":true,\"data\":%s,\"hints\":[", data_json ? data_json : "null");
    for (size_t i = 0; i < n; i++) off += snprintf(buf + off, cap - off, "%s\"%s\"", i ? "," : "", hints[i]);
    snprintf(buf + off, cap - off, "]}");
    if (cancelled) {
        free(buf);
        buf = dup_str("{\"ok\":false,\"kind\":\"CANCELLED\",\"message\":\"cancelled\"}");
    }
    consume(call, buf);
    if (cancelled) { set_error("已取消"); return AM_ERR_ALREADY_COMPLETED; }
    return AM_OK;
}

/* v9：在 am_call_complete 的结果 JSON 末尾追加 status / stateResource / summary / annotations。 */
AmStatus am_call_complete_ex(AmCall *call, const AmCallResult *r) {
    if (!call) return AM_ERR_INVALID_ARGUMENT;
    if (!r) return am_call_complete(call, NULL, NULL, 0);
    if (r->annotations_json && strcmp(r->annotations_json, "{bad") == 0) { set_error("非法 JSON"); return AM_ERR_INVALID_JSON; }
    if (r->data_json && strcmp(r->data_json, "{bad") == 0) { set_error("非法 JSON"); return AM_ERR_INVALID_JSON; }
    int index = call->index;
    AmStatus st = am_call_complete(call, r->data_json, r->state_hints, r->state_hints_len);
    if (st != AM_OK) return st;
    lock_global();
    char *base = g_results[index];
    size_t cap = strlen(base) + 128 + (r->state_resource ? strlen(r->state_resource) : 0) +
                 (r->summary ? strlen(r->summary) : 0) + (r->annotations_json ? strlen(r->annotations_json) : 0);
    char *buf = malloc(cap);
    int off = snprintf(buf, cap, "%.*s,\"status\":%d", (int)(strlen(base) - 1), base, (int)r->status);
    if (r->state_resource) off += snprintf(buf + off, cap - off, ",\"stateResource\":\"%s\"", r->state_resource);
    if (r->summary) off += snprintf(buf + off, cap - off, ",\"summary\":\"%s\"", r->summary);
    if (r->annotations_json) off += snprintf(buf + off, cap - off, ",\"annotations\":%s", r->annotations_json);
    snprintf(buf + off, cap - off, "}");
    g_results[index] = buf;
    unlock_global();
    free(base);
    return AM_OK;
}

AmStatus am_call_fail(AmCall *call, const char *kind, const char *message) {
    if (!call || !kind || !message) return AM_ERR_INVALID_ARGUMENT;
    int cancelled = am_call_is_cancelled(call);
    size_t cap = 64 + strlen(kind) + strlen(message);
    char *buf = malloc(cap);
    snprintf(buf, cap, "{\"ok\":false,\"kind\":\"%s\",\"message\":\"%s\"}", cancelled ? "CANCELLED" : kind,
             message);
    consume(call, buf);
    if (cancelled) return AM_ERR_ALREADY_COMPLETED;
    return AM_OK;
}

AmStatus am_call_fail_with_details(AmCall *call, const char *kind, const char *message, const char *details_json) {
    if (!call || !kind || !message) return AM_ERR_INVALID_ARGUMENT;
    if (!details_json) return am_call_fail(call, kind, message);
    if (strcmp(details_json, "{bad") == 0) { set_error("非法 JSON"); return AM_ERR_INVALID_JSON; }
    int cancelled = am_call_is_cancelled(call);
    size_t cap = 96 + strlen(kind) + strlen(message) + strlen(details_json);
    char *buf = malloc(cap);
    snprintf(buf, cap, "{\"ok\":false,\"kind\":\"%s\",\"message\":\"%s\",\"details\":%s}",
             cancelled ? "CANCELLED" : kind, message, details_json);
    consume(call, buf);
    return cancelled ? AM_ERR_ALREADY_COMPLETED : AM_OK;
}
/* v11：结果记为 {"ok":false,"kind":"USER_ACTION_REQUIRED","message",["reason"],["uri"]}（NULL 的字段省略）。 */
AmStatus am_call_fail_user_action(AmCall *call, const char *message, const char *reason, const char *uri) {
    if (!call) return AM_ERR_INVALID_ARGUMENT;
    if (!message) message = "";
    int cancelled = am_call_is_cancelled(call);
    size_t cap = 128 + strlen(message) + (reason ? strlen(reason) : 0) + (uri ? strlen(uri) : 0);
    char *buf = malloc(cap);
    int n = snprintf(buf, cap, "{\"ok\":false,\"kind\":\"%s\",\"message\":\"%s\"",
                     cancelled ? "CANCELLED" : "USER_ACTION_REQUIRED", message);
    if (reason) n += snprintf(buf + n, cap - n, ",\"reason\":\"%s\"", reason);
    if (uri) n += snprintf(buf + n, cap - n, ",\"uri\":\"%s\"", uri);
    snprintf(buf + n, cap - n, "}");
    consume(call, buf);
    return cancelled ? AM_ERR_ALREADY_COMPLETED : AM_OK;
}
/* v10：记录进度，"progress|total|message"（total 未知为 -，message 为 NULL 时为空），以换行分隔累积。 */
static char g_progress[4096];
AmStatus am_call_progress(const AmCall *call, double progress, double total, const char *message) {
    if (!call) return AM_ERR_INVALID_ARGUMENT;
    char line[512];
    if (total >= 0) snprintf(line, sizeof line, "%g|%g|%s\n", progress, total, message ? message : "");
    else snprintf(line, sizeof line, "%g|-|%s\n", progress, message ? message : "");
    lock_global();
    strncat(g_progress, line, sizeof g_progress - strlen(g_progress) - 1);
    unlock_global();
    return AM_OK;
}
char *fake_progress(void) {
    lock_global();
    char *s = dup_str(g_progress);
    g_progress[0] = 0;
    unlock_global();
    return s;
}
AmStatus am_call_hold(const AmCall *call, AmHold **out) {
    if (!call || !out) return AM_ERR_INVALID_ARGUMENT;
    *out = new_hold();
    return AM_OK;
}

/* ------------------------------------------------------------------ 读取 */

const char *am_read_resource_name(const AmRead *read) { return read->name; }
AmStatus am_read_complete(AmRead *read, const char *contents_json) {
    if (!read || !contents_json) return AM_ERR_INVALID_ARGUMENT;
    size_t cap = 32 + strlen(contents_json);
    char *buf = malloc(cap);
    snprintf(buf, cap, "{\"ok\":true,\"contents\":%s}", contents_json);
    lock_global();
    g_results[read->index] = buf;
    unlock_global();
    free(read->name);
    free(read);
    return AM_OK;
}
AmStatus am_read_fail(AmRead *read, const char *kind, const char *message) {
    if (!read || !kind || !message) return AM_ERR_INVALID_ARGUMENT;
    size_t cap = 64 + strlen(kind) + strlen(message);
    char *buf = malloc(cap);
    snprintf(buf, cap, "{\"ok\":false,\"kind\":\"%s\",\"message\":\"%s\"}", kind, message);
    lock_global();
    g_results[read->index] = buf;
    unlock_global();
    free(read->name);
    free(read);
    return AM_OK;
}

static void finish_read(AmRead *read, char *buf) {
    lock_global();
    g_results[read->index] = buf;
    unlock_global();
    free(read->name);
    free(read);
}

/* v12：结果记为 {"ok":false,"kind","message","details"}；"{bad" 视为非法 JSON（不消费 read）。 */
AmStatus am_read_fail_with_details(AmRead *read, const char *kind, const char *message, const char *details_json) {
    if (!read || !kind || !message) return AM_ERR_INVALID_ARGUMENT;
    if (!details_json) return am_read_fail(read, kind, message);
    if (strcmp(details_json, "{bad") == 0) { set_error("非法 JSON"); return AM_ERR_INVALID_JSON; }
    size_t cap = 96 + strlen(kind) + strlen(message) + strlen(details_json);
    char *buf = malloc(cap);
    snprintf(buf, cap, "{\"ok\":false,\"kind\":\"%s\",\"message\":\"%s\",\"details\":%s}", kind, message,
             details_json);
    finish_read(read, buf);
    return AM_OK;
}

/* v12：结果记为 {"ok":false,"kind":"USER_ACTION_REQUIRED","message",["reason"],["uri"]}（NULL 的字段省略）。 */
AmStatus am_read_fail_user_action(AmRead *read, const char *message, const char *reason, const char *uri) {
    if (!read) return AM_ERR_INVALID_ARGUMENT;
    if (!message) message = "";
    size_t cap = 128 + strlen(message) + (reason ? strlen(reason) : 0) + (uri ? strlen(uri) : 0);
    char *buf = malloc(cap);
    int n = snprintf(buf, cap, "{\"ok\":false,\"kind\":\"USER_ACTION_REQUIRED\",\"message\":\"%s\"", message);
    if (reason) n += snprintf(buf + n, cap - n, ",\"reason\":\"%s\"", reason);
    if (uri) n += snprintf(buf + n, cap - n, ",\"uri\":\"%s\"", uri);
    snprintf(buf + n, cap - n, "}");
    finish_read(read, buf);
    return AM_OK;
}

/* ------------------------------------------------------------------ 测试驱动 */

typedef struct { ToolRec *t; AmCall *call; } InvokeJob;
static void invoke_job(void *p) {
    InvokeJob *j = p;
    j->t->handler(j->t->user_data, j->call);
    free(j);
}

/* 在新线程上调用工具（idempotency_key 为 NULL = 无幂等键）。返回结果槽位；工具不存在时返回 -1。 */
int fake_invoke_with_key(const char *tool, const char *args_json, const char *idempotency_key) {
    AmClient *c = g_client;
    ToolRec *t = find_tool(c, tool);
    if (!t || !t->enabled) return -1;
    lock_global();
    int idx = g_n_calls++;
    unlock_global();
    AmCall *call = calloc(1, sizeof *call);
    call->index = idx;
    char id[32];
    snprintf(id, sizeof id, "call-%d", idx);
    call->id = dup_str(id);
    call->tool_name = dup_str(tool);
    call->args = dup_str(args_json);
    call->idempotency_key = idempotency_key ? dup_str(idempotency_key) : NULL;
    g_calls[idx] = call;
    InvokeJob *j = malloc(sizeof *j);
    j->t = t;
    j->call = call;
    run_on_thread(invoke_job, j);
    return idx;
}

/* 在新线程上调用工具（无幂等键）。 */
int fake_invoke(const char *tool, const char *args_json) { return fake_invoke_with_key(tool, args_json, NULL); }

typedef struct { ResRec *r; AmRead *read; } ReadJob;
static void read_job(void *p) {
    ReadJob *j = p;
    j->r->reader(j->r->user_data, j->read);
    free(j);
}
int fake_read(const char *name) {
    AmClient *c = g_client;
    ResRec *r = NULL;
    for (int i = 0; i < c->n_res; i++)
        if (!c->res[i]->disposed && scope_alive(c->res[i]->scope) && strcmp(c->res[i]->name, name) == 0) r = c->res[i];
    if (!r) return -1;
    lock_global();
    int idx = g_n_calls++;
    unlock_global();
    AmRead *read = calloc(1, sizeof *read);
    read->index = idx;
    read->name = dup_str(name);
    ReadJob *j = malloc(sizeof *j);
    j->r = r;
    j->read = read;
    run_on_thread(read_job, j);
    return idx;
}

typedef struct { AmCancelFn f; void *ud; int reason; } CancelJob;
static void cancel_job(void *p) {
    CancelJob *j = p;
    j->f(j->ud, (AmCancelReason)j->reason);
    free(j);
}
/* 取消进行中的调用；已完成时返回 0。 */
int fake_cancel(int idx, int reason) {
    lock_global();
    AmCall *call = g_calls[idx];
    if (!call || call->cancelled) { unlock_global(); return 0; }
    call->cancelled = 1;
    AmCancelFn f = call->on_cancel;
    void *ud = call->cancel_ud;
    unlock_global();
    if (f) {
        CancelJob *j = malloc(sizeof *j);
        j->f = f; j->ud = ud; j->reason = reason;
        run_on_thread(cancel_job, j);
    }
    return 1;
}

/* 结果 JSON（需 am_string_free）；未完成时返回 NULL。 */
char *fake_result(int idx) {
    lock_global();
    char *r = dup_str(g_results[idx]);
    unlock_global();
    return r;
}

void fake_emit_state(int status, uint64_t retry, const char *reason) { emit_state(g_client, status, retry, reason); }
void fake_emit_state_code(int status, uint64_t retry, const char *reason, const char *code) {
    emit_state_code(g_client, status, retry, reason, code);
}

typedef struct { AmClient *c; char *token; } PairJob;
static void pair_job(void *p) {
    PairJob *j = p;
    if (j->c->cb.on_paired) j->c->cb.on_paired(j->c->cb.user_data, j->token);
    else am_string_free(j->token);
    free(j);
}
void fake_pair(const char *token) {
    AmClient *c = g_client;
    lock_global();
    free(c->token);
    c->token = dup_str(token);
    unlock_global();
    PairJob *j = malloc(sizeof *j);
    j->c = c;
    j->token = give_owned(token);
    run_on_thread(pair_job, j);
}

typedef struct { AmClient *c; int level; char *message; } LogJob;
static void log_job(void *p) {
    LogJob *j = p;
    if (j->c->cb.on_log) j->c->cb.on_log(j->c->cb.user_data, (AmLogLevel)j->level, j->message);
    else am_string_free(j->message);
    free(j);
}
/* 在分发线程上输出一条日志。 */
void fake_log(int level, const char *message) {
    LogJob *j = malloc(sizeof *j);
    j->c = g_client;
    j->level = level;
    j->message = give_owned(message);
    run_on_thread(log_job, j);
}
/* 交给回调方但尚未 am_string_free 的字符串数量。 */
int fake_owned_outstanding(void) {
    lock_global();
    int n = 0;
    for (int i = 0; i < (int)(sizeof g_owned / sizeof g_owned[0]); i++)
        if (g_owned[i]) n++;
    unlock_global();
    return n;
}
/* 交给回调方的字符串总数。 */
int fake_owned_total(void) {
    lock_global();
    int n = g_owned_total;
    unlock_global();
    return n;
}

int fake_free_count(void) {
    lock_global();
    int n = g_free_count;
    unlock_global();
    return n;
}
int fake_visibility(void) { return g_visibility; }
int fake_focused(void) { return g_focused; }
int fake_notify_count(void) { return g_notify_count; }
/* 资源注册时的 realtime：1 / 0；找不到（或已注销）时 -1。 */
int fake_resource_realtime(const char *name) {
    AmClient *c = g_client;
    if (!c || !name) return -1;
    for (int i = c->n_res - 1; i >= 0; i--)
        if (!c->res[i]->disposed && strcmp(c->res[i]->name, name) == 0) return c->res[i]->realtime;
    return -1;
}
/* v13：资源注册时的内容标注（需 am_string_free）；未声明或找不到时 NULL。 */
char *fake_resource_annotations(const char *name) {
    AmClient *c = g_client;
    if (!c || !name) return NULL;
    for (int i = c->n_res - 1; i >= 0; i--)
        if (!c->res[i]->disposed && strcmp(c->res[i]->name, name) == 0) return dup_str(c->res[i]->annotations);
    return NULL;
}
/* 工具当前的 enabled / risk / 描述，供测试断言 update。 */
int fake_tool_enabled(const char *name) { ToolRec *t = find_tool(g_client, name); return t ? t->enabled : -1; }
char *fake_tool_description(const char *name) { ToolRec *t = find_tool(g_client, name); return t ? dup_str(t->description) : NULL; }
/* v9：工具当前的注解与 outputSchema，"<annotations 或 null>|<outputSchema 或 null>"（需 am_string_free）。 */
char *fake_tool_options(const char *name) {
    ToolRec *t = find_tool(g_client, name);
    if (!t) return NULL;
    const char *a = t->annotations ? t->annotations : "null";
    const char *o = t->output_schema ? t->output_schema : "null";
    size_t cap = strlen(a) + strlen(o) + 2;
    char *buf = malloc(cap);
    snprintf(buf, cap, "%s|%s", a, o);
    return buf;
}

/* v14：工具当前的 surface 与页面，"<0|1>|<page 或 null>"（需 am_string_free）。 */
char *fake_tool_view(const char *name) {
    ToolRec *t = find_tool(g_client, name);
    if (!t) return NULL;
    const char *p = t->page ? t->page : "null";
    size_t cap = strlen(p) + 16;
    char *buf = malloc(cap);
    snprintf(buf, cap, "%d|%s", t->surface, p);
    return buf;
}

/* v15：工具当前的 background_tool（需 am_string_free）；未声明或工具不存在时返回 NULL。 */
char *fake_tool_background(const char *name) {
    ToolRec *t = find_tool(g_client, name);
    return t ? dup_str(t->background_tool) : NULL;
}
/* v18：工具当前的调度声明 "<concurrency>|<exclusive 或 ->"（需 am_string_free）；工具不存在时返回 NULL。 */
char *fake_tool_schedule(const char *name) {
    ToolRec *t = find_tool(g_client, name);
    char buf[96];
    if (t) snprintf(buf, sizeof buf, "%u|%s", (unsigned)t->concurrency, t->exclusive ? t->exclusive : "-");
    return t ? dup_str(buf) : NULL;
}

/* 最近一次 am_client_new 的总览（需 am_string_free）；没有时返回 NULL。 */
char *fake_overview(void) { return dup_str(g_overview); }
