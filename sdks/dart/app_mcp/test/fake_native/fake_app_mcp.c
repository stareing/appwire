/*
 * 测试用的假 libapp_mcp：按 app_mcp.h 实现全部函数，但不连接 Host。
 *
 * 所有回调都在新建的 pthread 上触发（模拟库的分发线程）。按 API 版本 2，状态 / 配对 / 日志
 * 回调中的字符串归接收方所有；假库记录这些字符串，am_string_free 时注销，
 * fake_owned_outstanding 返回尚未释放的数量，用来检验 Dart 封装会释放它们。
 * 额外导出 fake_* 函数供测试驱动调用、取消与状态变化。
 *
 * v3（生命周期）：am_client_new_ex 记录生命周期配置（fake_lifecycle），handle_wake 识别
 * "app-mcp-wake:" 前缀，wake / sleep / connect_now 发出状态变化，hold 计数（fake_hold_count），
 * fake_idle_exit 在其他线程触发 on_idle_exit。
 *
 * 编译：cc -shared -fPIC -o libfake_app_mcp.so fake_app_mcp.c -lpthread
 */
#include "../../../../../bindings/c/include/app_mcp.h"

#include <pthread.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static pthread_mutex_t g_lock = PTHREAD_MUTEX_INITIALIZER;
static int g_free_count = 0;
static __thread char g_last_error[256];

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
} ToolRec;

typedef struct ResRec {
    char *name;
    ScopeRec *scope;
    int disposed;
    AmReadFn reader;
    void *user_data;
    AmFreeFn free_user_data;
    int freed;
} ResRec;

#define MAX_ITEMS 256

struct AmClient {
    AmClientCallbacks cb;
    char *token;
    char *instance_id;
    int status;
    uint64_t retry;
    char *reason;
    ScopeRec root;
    ToolRec *tools[MAX_ITEMS];
    int n_tools;
    ResRec *res[MAX_ITEMS];
    int n_res;
    int stopped;
    AmIdleExitFn on_idle_exit;
};
struct AmScope { AmClient *client; ScopeRec *rec; };
struct AmTool { AmClient *client; ToolRec *rec; };
struct AmResource { AmClient *client; ResRec *rec; };

struct AmCall {
    int index;
    char *id, *tool_name, *args;
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
static void *job_main(void *p) {
    Job j = *(Job *)p;
    free(p);
    j.f(j.arg);
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

typedef struct { AmFreeFn f; void *ud; } FreeJob;
static void free_job(void *p) {
    FreeJob *j = p;
    j->f(j->ud);
    pthread_mutex_lock(&g_lock);
    g_free_count++;
    pthread_mutex_unlock(&g_lock);
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
    pthread_mutex_lock(&g_lock);
    for (int i = 0; i < (int)(sizeof g_owned / sizeof g_owned[0]); i++) {
        if (!g_owned[i]) { g_owned[i] = p; break; }
    }
    g_owned_total++;
    pthread_mutex_unlock(&g_lock);
    return p;
}
void am_string_free(char *s) {
    if (!s) return;
    pthread_mutex_lock(&g_lock);
    for (int i = 0; i < (int)(sizeof g_owned / sizeof g_owned[0]); i++) {
        if (g_owned[i] == s) { g_owned[i] = NULL; break; }
    }
    pthread_mutex_unlock(&g_lock);
    free(s);
}

/* ------------------------------------------------------------------ 客户端 */

static char *g_lifecycle = NULL; /* 最近一次配置的生命周期，见 am_client_new_ex */
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
    if (options) {
        (*out)->on_idle_exit = options->on_idle_exit;
        const AmLifecycle *l = options->lifecycle;
        char buf[512];
        if (l) {
            snprintf(buf, sizeof buf, "%d|%llu|%llu|%llu|%d|%d|%s|%d|%u", (int)l->mode,
                     (unsigned long long)l->idle_timeout_ms, (unsigned long long)l->hidden_idle_timeout_ms,
                     (unsigned long long)l->grace_ms, (int)l->residency, (int)l->wake_kind,
                     l->wake_target ? l->wake_target : "-", l->wake_background ? 1 : 0, options->connect_timeout_ms);
        } else {
            snprintf(buf, sizeof buf, "-|%u", options->connect_timeout_ms);
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
static void emit_state(AmClient *c, int status, uint64_t retry, const char *reason) {
    pthread_mutex_lock(&g_lock);
    c->status = status;
    c->retry = retry;
    free(c->reason);
    c->reason = dup_str(reason);
    pthread_mutex_unlock(&g_lock);
    StateJob *j = malloc(sizeof *j);
    j->c = c; j->status = status; j->retry = retry; j->reason = give_owned(reason);
    run_on_thread(state_job, j);
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
    pthread_mutex_lock(&g_lock);
    *status = (AmStateStatus)c->status;
    if (retry) *retry = c->retry;
    if (reason) *reason = c->status == AM_STATE_REJECTED ? dup_str(c->reason ? c->reason : "") : NULL;
    pthread_mutex_unlock(&g_lock);
    return AM_OK;
}
char *am_client_instance_id(const AmClient *c) { return dup_str(c->instance_id); }
char *am_client_token(const AmClient *c) {
    pthread_mutex_lock(&g_lock);
    char *t = dup_str(c->token);
    pthread_mutex_unlock(&g_lock);
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
    pthread_mutex_lock(&g_lock);
    free(g_last_sleep);
    g_last_sleep = dup_str(buf);
    pthread_mutex_unlock(&g_lock);
    int was = c->status != AM_STATE_DORMANT;
    emit_state(c, AM_STATE_DORMANT, 0, NULL);
    if (changed) *changed = was;
    return AM_OK;
}
AmStatus am_client_sleep(AmClient *c, bool *changed) { return am_client_sleep_with_reason(c, AM_SLEEP_REASON_APP, changed); }
static AmHold *new_hold(void) {
    AmHold *h = calloc(1, sizeof *h);
    pthread_mutex_lock(&g_lock);
    g_holds++;
    pthread_mutex_unlock(&g_lock);
    return h;
}
AmStatus am_client_hold(AmClient *c, AmHold **out) {
    if (!c || !out) return AM_ERR_INVALID_ARGUMENT;
    *out = new_hold();
    return AM_OK;
}
void am_hold_release(AmHold *h) {
    if (!h) return;
    pthread_mutex_lock(&g_lock);
    g_holds--;
    pthread_mutex_unlock(&g_lock);
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
    pthread_mutex_lock(&g_lock);
    int n = g_holds;
    pthread_mutex_unlock(&g_lock);
    return n;
}
/* 最近一次生命周期配置（需 am_string_free）；没有 options 时返回 NULL。 */
char *fake_lifecycle(void) { return dup_str(g_lifecycle); }
/* 最近一次 sleep 的原因（需 am_string_free）。 */
char *fake_last_sleep(void) {
    pthread_mutex_lock(&g_lock);
    char *s = dup_str(g_last_sleep);
    pthread_mutex_unlock(&g_lock);
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
bool am_call_is_cancelled(const AmCall *call) {
    pthread_mutex_lock(&g_lock);
    int c = call->cancelled;
    pthread_mutex_unlock(&g_lock);
    return c;
}
AmStatus am_call_set_cancel_callback(AmCall *call, AmCancelFn on_cancel, void *ud, AmFreeFn free_ud) {
    if (!call || !on_cancel) return AM_ERR_INVALID_ARGUMENT;
    pthread_mutex_lock(&g_lock);
    call->on_cancel = on_cancel;
    call->cancel_ud = ud;
    call->cancel_free = free_ud;
    int cancelled = call->cancelled;
    pthread_mutex_unlock(&g_lock);
    if (cancelled) on_cancel(ud, AM_CANCEL_REQUESTED);
    return AM_OK;
}

static void consume(AmCall *call, char *result) {
    pthread_mutex_lock(&g_lock);
    g_results[call->index] = result;
    g_calls[call->index] = NULL;
    AmFreeFn f = call->cancel_free;
    void *ud = call->cancel_ud;
    pthread_mutex_unlock(&g_lock);
    release_user_data(f, ud);
    free(call->id);
    free(call->tool_name);
    memset(call->args, 0, strlen(call->args));
    free(call->args);
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
    pthread_mutex_lock(&g_lock);
    g_results[read->index] = buf;
    pthread_mutex_unlock(&g_lock);
    free(read->name);
    free(read);
    return AM_OK;
}
AmStatus am_read_fail(AmRead *read, const char *kind, const char *message) {
    if (!read || !kind || !message) return AM_ERR_INVALID_ARGUMENT;
    size_t cap = 64 + strlen(kind) + strlen(message);
    char *buf = malloc(cap);
    snprintf(buf, cap, "{\"ok\":false,\"kind\":\"%s\",\"message\":\"%s\"}", kind, message);
    pthread_mutex_lock(&g_lock);
    g_results[read->index] = buf;
    pthread_mutex_unlock(&g_lock);
    free(read->name);
    free(read);
    return AM_OK;
}

/* ------------------------------------------------------------------ 测试驱动 */

typedef struct { ToolRec *t; AmCall *call; } InvokeJob;
static void invoke_job(void *p) {
    InvokeJob *j = p;
    j->t->handler(j->t->user_data, j->call);
    free(j);
}

/* 在新线程上调用工具。返回结果槽位；工具不存在时返回 -1。 */
int fake_invoke(const char *tool, const char *args_json) {
    AmClient *c = g_client;
    ToolRec *t = find_tool(c, tool);
    if (!t || !t->enabled) return -1;
    pthread_mutex_lock(&g_lock);
    int idx = g_n_calls++;
    pthread_mutex_unlock(&g_lock);
    AmCall *call = calloc(1, sizeof *call);
    call->index = idx;
    char id[32];
    snprintf(id, sizeof id, "call-%d", idx);
    call->id = dup_str(id);
    call->tool_name = dup_str(tool);
    call->args = dup_str(args_json);
    g_calls[idx] = call;
    InvokeJob *j = malloc(sizeof *j);
    j->t = t;
    j->call = call;
    run_on_thread(invoke_job, j);
    return idx;
}

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
    pthread_mutex_lock(&g_lock);
    int idx = g_n_calls++;
    pthread_mutex_unlock(&g_lock);
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
    pthread_mutex_lock(&g_lock);
    AmCall *call = g_calls[idx];
    if (!call || call->cancelled) { pthread_mutex_unlock(&g_lock); return 0; }
    call->cancelled = 1;
    AmCancelFn f = call->on_cancel;
    void *ud = call->cancel_ud;
    pthread_mutex_unlock(&g_lock);
    if (f) {
        CancelJob *j = malloc(sizeof *j);
        j->f = f; j->ud = ud; j->reason = reason;
        run_on_thread(cancel_job, j);
    }
    return 1;
}

/* 结果 JSON（需 am_string_free）；未完成时返回 NULL。 */
char *fake_result(int idx) {
    pthread_mutex_lock(&g_lock);
    char *r = dup_str(g_results[idx]);
    pthread_mutex_unlock(&g_lock);
    return r;
}

void fake_emit_state(int status, uint64_t retry, const char *reason) { emit_state(g_client, status, retry, reason); }

typedef struct { AmClient *c; char *token; } PairJob;
static void pair_job(void *p) {
    PairJob *j = p;
    if (j->c->cb.on_paired) j->c->cb.on_paired(j->c->cb.user_data, j->token);
    else am_string_free(j->token);
    free(j);
}
void fake_pair(const char *token) {
    AmClient *c = g_client;
    pthread_mutex_lock(&g_lock);
    free(c->token);
    c->token = dup_str(token);
    pthread_mutex_unlock(&g_lock);
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
    pthread_mutex_lock(&g_lock);
    int n = 0;
    for (int i = 0; i < (int)(sizeof g_owned / sizeof g_owned[0]); i++)
        if (g_owned[i]) n++;
    pthread_mutex_unlock(&g_lock);
    return n;
}
/* 交给回调方的字符串总数。 */
int fake_owned_total(void) {
    pthread_mutex_lock(&g_lock);
    int n = g_owned_total;
    pthread_mutex_unlock(&g_lock);
    return n;
}

int fake_free_count(void) {
    pthread_mutex_lock(&g_lock);
    int n = g_free_count;
    pthread_mutex_unlock(&g_lock);
    return n;
}
int fake_visibility(void) { return g_visibility; }
int fake_focused(void) { return g_focused; }
int fake_notify_count(void) { return g_notify_count; }
/* 工具当前的 enabled / risk / 描述，供测试断言 update。 */
int fake_tool_enabled(const char *name) { ToolRec *t = find_tool(g_client, name); return t ? t->enabled : -1; }
char *fake_tool_description(const char *name) { ToolRec *t = find_tool(g_client, name); return t ? dup_str(t->description) : NULL; }

/* 最近一次 am_client_new 的总览（需 am_string_free）；没有时返回 NULL。 */
char *fake_overview(void) { return dup_str(g_overview); }
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
    default: return 0;
    }
}
