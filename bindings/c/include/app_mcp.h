/*
 * app_mcp.h —— app-mcp 原生 SDK 的 C 接口（契约）。
 *
 * 由 bindings/c（Rust cdylib / staticlib，库名 app_mcp）实现，是 C、C++、
 * C#（P/Invoke）、Dart（dart:ffi）等语言绑定的共同基础。语义与 crates/native 一致。
 *
 * 约定
 * - 所有字符串为 UTF-8、以 NUL 结尾。
 * - 传入的字符串由调用方持有，函数返回后即可释放（库会复制）。
 * - 返回 char* 的函数，由调用方用 am_string_free 释放。
 * - 返回 const char* 的函数，指针归库所有，有效期见各函数说明。
 * - 函数返回 AmStatus；非 AM_OK 时可立即用 am_last_error_message 取得错误说明（线程局部）。
 * - 所有函数线程安全。Rust panic 不会跨越 FFI 边界，转换为 AM_ERR_PANIC。
 * - 回调在库的分发线程上执行，不持有内部锁，必须尽快返回。需要 UI 线程执行的工作，
 *   由调用方切换线程后再完成调用（am_call_complete / am_call_fail 可在任意线程调用）。
 * - user_data 与 free_user_data：库在不再使用 user_data 时调用 free_user_data（可为 NULL）。
 *   user_data 传入后无论函数成功与否都归库所有；失败时库在函数返回前调用 free_user_data。
 * - 句柄（AmClient、AmScope、AmTool、AmResource）用对应的 *_free 释放。释放句柄不会注销
 *   工具或资源；注销用 *_dispose。
 *
 * 版本历史
 * - v2：回调中的字符串归回调方所有（am_string_free）。
 * - v3（生命周期，spec/lifecycle.md）：只做新增，v2 的结构体布局与函数签名不变。
 *   · AmClientOptions + am_client_new_ex：生命周期策略（AmLifecycle）、connect_timeout_ms、
 *     idle-exit 回调（AmIdleExitFn）。AmClientConfig / AmClientCallbacks 布局保持 v2 不变，
 *     扩展项放在带 struct_size 的新结构体中，便于以后继续追加字段。
 *   · am_lifecycle_init：按规范默认值初始化 AmLifecycle。
 *   · am_client_handle_wake / am_client_wake / am_client_wake_with_reason / am_client_connect_now /
 *     am_client_sleep / am_client_sleep_with_reason / am_client_hold（AmHold + am_hold_release）/
 *     am_client_tools_hash / am_parse_wake_token。
 *   · am_call_hold、am_call_fail_with_details。
 * - v4（本地 IPC 传输，spec/protocol.md 第 1 节）：布局与签名不变，只扩展 host_url 的取值与缺省值。
 * - v5（Host 身份与登记文件，spec/protocol.md 1.6、1.7）：布局与签名不变。新增状态 AM_STATE_HOST_MISMATCH = 10
 *   （对端不是 app-mcp，或属于其他用户；reason 非 NULL）；host_url 缺省值的解析顺序加入登记文件，
 *   WebSocket 缺省地址改为 "ws://127.0.0.1:7717/app"。
 * - v6（诊断，spec/protocol.md 第 10 节）：布局与签名不变，只新增函数 am_client_state_code（当前状态的错误码，
 *   如 "HOST_NOT_RUNNING"、"HOST_NOT_APP_MCP"）与 am_client_connection_id（Host 分配的连接 ID）；
 *   连接期间 on_log 收到的日志以 "[连接 ID] " 开头。
 * - v7（4e 生命周期功耗，spec/lifecycle.md 第 11 节）：只在 AmClientOptions 末尾追加 heartbeat（AmHeartbeatMode）、
 *   host_absent_retries、legacy_timers（按 struct_size 读取，旧调用方不受影响）；新增枚举 AmHeartbeatMode。
 *   默认行为变化：租约与空闲计时并行；idle / on-demand 下连续 3 次 HOST_NOT_RUNNING 后进入 DORMANT；
 *   本地 IPC / 桌面本机回环不发心跳。legacy_timers = true 恢复旧行为。
 *   （AM_API_VERSION 只在不兼容的布局 / 签名变化时递增，v4、v5、v6、v7 仍为 3。）
 *
 * 端点（AmClientConfig.host_url）
 *   "unix:<绝对路径>"（Linux / macOS）、"pipe:\\.\pipe\<名称>"（Windows，C 字符串中需转义）、
 *   "ws://…" / "wss://…"（App 连接路径为 /app，如 "ws://127.0.0.1:7717/app"）。NULL 时：
 *   环境变量 APP_MCP_ENDPOINT（非空时原样使用）→ 登记文件 ~/.app-mcp/run/endpoints.json（运行中的 Host 写下的
 *   实际端点；APP_MCP_HOME 可改配置目录）→ 平台默认本地 IPC 端点
 *   （Linux $XDG_RUNTIME_DIR/app-mcp/hub.sock，否则 ~/.app-mcp/run/hub.sock；macOS ~/.app-mcp/run/hub.sock；
 *   Windows \\.\pipe\app-mcp-<当前用户 SID>）→ "ws://127.0.0.1:7717/app"（Android / iOS）。
 *   格式不合法或本平台不支持该形式时 am_client_new 返回 AM_ERR_INVALID_CONFIG；连不上时按退避重连同一端点。
 */
#ifndef APP_MCP_H
#define APP_MCP_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define AM_API_VERSION 3

/* ---------------------------------------------------------------------------
 * 状态码与枚举
 * ------------------------------------------------------------------------- */

typedef enum AmStatus {
    AM_OK = 0,
    AM_ERR_INVALID_ARGUMENT = 1,   /* 空指针、非法 UTF-8、非法枚举值 */
    AM_ERR_INVALID_NAME = 2,
    AM_ERR_INVALID_SCHEMA = 3,
    AM_ERR_DUPLICATE_NAME = 4,
    AM_ERR_INVALID_JSON = 5,
    AM_ERR_INVALID_CONFIG = 6,
    AM_ERR_ALREADY_COMPLETED = 7,
    AM_ERR_DISPOSED = 8,
    AM_ERR_STOPPED = 9,
    AM_ERR_INTERNAL = 10,
    AM_ERR_PANIC = 11
} AmStatus;

typedef enum AmClientKind { AM_CLIENT_NATIVE = 0, AM_CLIENT_HYBRID = 1 } AmClientKind;

typedef enum AmRisk {
    AM_RISK_READ = 0,
    AM_RISK_WRITE = 1,
    AM_RISK_DESTRUCTIVE = 2,
    AM_RISK_PAYMENT = 3,
    AM_RISK_OS_SENSITIVE = 4
} AmRisk;

typedef enum AmActivation {
    AM_ACTIVATION_NONE = -1,
    AM_ACTIVATION_HEADLESS = 0,
    AM_ACTIVATION_BACKGROUND = 1,
    AM_ACTIVATION_FOREGROUND = 2
} AmActivation;

typedef enum AmVisibility { AM_VISIBLE = 0, AM_HIDDEN = 1, AM_FROZEN = 2 } AmVisibility;

typedef enum AmCancelReason {
    AM_CANCEL_REQUESTED = 0,
    AM_CANCEL_TIMEOUT = 1,
    AM_CANCEL_DISCONNECTED = 2,
    AM_CANCEL_STOPPED = 3
} AmCancelReason;

typedef enum AmStateStatus {
    AM_STATE_IDLE = 0,
    AM_STATE_CONNECTING = 1,
    AM_STATE_HANDSHAKING = 2,
    AM_STATE_PENDING_PAIRING = 3,
    AM_STATE_CONNECTED = 4,
    AM_STATE_BACKOFF = 5,
    AM_STATE_REJECTED = 6,
    AM_STATE_STOPPED = 7,
    /* 已休眠：无连接、无定时器，等待唤醒（spec/lifecycle.md） */
    AM_STATE_DORMANT = 8,
    /* 收到唤醒后正在回连 */
    AM_STATE_WAKING = 9,
    /* 对端不是期望的 Host（不是 app-mcp，或属于其他用户；spec/protocol.md 1.6）：不再自动重连，
     * am_client_wake / am_client_connect_now 时再试一次；reason 为原因 */
    AM_STATE_HOST_MISMATCH = 10
} AmStateStatus;

typedef enum AmLogLevel { AM_LOG_DEBUG = 0, AM_LOG_INFO = 1, AM_LOG_WARN = 2, AM_LOG_ERROR = 3 } AmLogLevel;

/* v3：生命周期（spec/lifecycle.md 第 3 节） */
/* v7：心跳策略（AmClientOptions.heartbeat）。 */
typedef enum AmHeartbeatMode {
    AM_HEARTBEAT_AUTO = 0,         /* 按传输：本地 IPC / 桌面本机回环不发，远程发 */
    AM_HEARTBEAT_ALWAYS = 1,
    AM_HEARTBEAT_OFF = 2
} AmHeartbeatMode;

typedef enum AmLifecycleMode {
    AM_LIFECYCLE_PERSISTENT = 0,   /* 不休眠（默认） */
    AM_LIFECYCLE_IDLE = 1,         /* 启动即连接，空闲后休眠 */
    AM_LIFECYCLE_ON_DEMAND = 2     /* 启动时不连接；唤醒 / connect_now 时连接，完成后经过 grace_ms 休眠 */
} AmLifecycleMode;

typedef enum AmResidency {
    AM_RESIDENCY_KEEP = 0,           /* 只断开连接 */
    AM_RESIDENCY_EXIT_WHEN_IDLE = 1, /* 由唤醒冷启动的进程，休眠后回调 on_idle_exit */
    AM_RESIDENCY_EXIT_ALWAYS = 2     /* 每次休眠后都回调 on_idle_exit（无界面辅助进程） */
} AmResidency;

typedef enum AmWakeKind {
    AM_WAKE_UNSET = -1,            /* 不上报唤醒描述（Host 回退到清单 launch） */
    AM_WAKE_NONE = 0,              /* 明确声明不可唤醒 */
    AM_WAKE_URI = 1,
    AM_WAKE_AUMID = 2,
    AM_WAKE_APPLE_EVENT = 3,
    AM_WAKE_DBUS = 4,
    AM_WAKE_ANDROID_INTENT = 5,
    AM_WAKE_WEB_URL = 6
} AmWakeKind;

typedef enum AmWakeReason {
    AM_WAKE_REASON_OS_ACTIVATION = 0,
    AM_WAKE_REASON_APP = 1,
    AM_WAKE_REASON_VISIBLE = 2,
    AM_WAKE_REASON_COLD_START = 3
} AmWakeReason;

typedef enum AmSleepReason {
    AM_SLEEP_REASON_IDLE = 0,
    AM_SLEEP_REASON_GRACE = 1,
    AM_SLEEP_REASON_BACKGROUND = 2,
    AM_SLEEP_REASON_APP = 3
} AmSleepReason;

/* ---------------------------------------------------------------------------
 * 不透明句柄
 * ------------------------------------------------------------------------- */

typedef struct AmClient AmClient;
typedef struct AmScope AmScope;
typedef struct AmTool AmTool;
typedef struct AmResource AmResource;
typedef struct AmCall AmCall;   /* 一次调用；由 am_call_complete / am_call_fail 消费 */
typedef struct AmRead AmRead;   /* 一次读取；由 am_read_complete / am_read_fail 消费 */
typedef struct AmHold AmHold;   /* v3：阻止自动休眠的持有；由 am_hold_release 释放 */

/* ---------------------------------------------------------------------------
 * 回调
 * ------------------------------------------------------------------------- */

typedef void (*AmFreeFn)(void *user_data);

/* 以下三个回调中的字符串归回调方所有（API 版本 2 起）：由库分配，回调方必须用 am_string_free 释放
 * （可以在回调返回后、任意线程释放，便于异步投递，如 dart:ffi NativeCallable.listener）。 */
/* 状态变化。retry_in_ms 仅 BACKOFF 时有意义（否则为 0）；reason 在 REJECTED / HOST_MISMATCH 时非 NULL，
 * v6 起 BACKOFF 在连接失败等情况下也带原因（错误码见 am_client_state_code）；
 * reason 为 NULL 或需由回调方用 am_string_free 释放。 */
typedef void (*AmStateFn)(void *user_data, AmStateStatus status, uint64_t retry_in_ms, char *reason);
/* 配对成功，App 应持久化 token。token 非 NULL，需由回调方用 am_string_free 释放。 */
typedef void (*AmPairedFn)(void *user_data, char *token);
/* 日志。message 非 NULL，需由回调方用 am_string_free 释放。 */
typedef void (*AmLogFn)(void *user_data, AmLogLevel level, char *message);
/* 工具调用。call 的所有权转移给回调方，必须最终调用 am_call_complete 或 am_call_fail 恰好一次。 */
typedef void (*AmToolFn)(void *user_data, AmCall *call);
/* 资源读取。read 的所有权转移给回调方，必须最终调用 am_read_complete 或 am_read_fail 恰好一次。 */
typedef void (*AmReadFn)(void *user_data, AmRead *read);
/* 调用被取消。 */
typedef void (*AmCancelFn)(void *user_data, AmCancelReason reason);
/* v3：已进入休眠，且驻留策略允许退出进程（residency 为 EXIT_WHEN_IDLE / EXIT_ALWAYS）。
 * App 自行决定是否退出；不要在回调中同步调用 am_client_free（先切换到其他线程）。 */
typedef void (*AmIdleExitFn)(void *user_data);

/* ---------------------------------------------------------------------------
 * 配置与定义
 * ------------------------------------------------------------------------- */

typedef struct AmClientConfig {
    const char *app_id;            /* 必填 */
    const char *app_name;          /* 必填 */
    const char *instance_id;       /* 可为 NULL：自动生成 */
    const char *host_url;          /* 可为 NULL：见下方“端点” */
    const char *app_version;       /* 可为 NULL */
    const char *instance_title;    /* 可为 NULL */
    const char *token;             /* 可为 NULL */
    const char *launch_token;      /* 可为 NULL：读取环境变量 APP_MCP_LAUNCH_TOKEN */
    AmClientKind client_kind;
    uint32_t max_concurrent_calls; /* 0 表示默认值 1 */
    const char *overview_summary;  /* 可为 NULL：无总览（≤ 100 字符） */
    const char *overview_body;     /* 可为 NULL（Markdown，≤ 2000 字符） */
    const char *overview_locale;   /* 可为 NULL，如 "zh-CN" */
} AmClientConfig;

typedef struct AmClientCallbacks {
    AmStateFn on_state;            /* 可为 NULL */
    AmPairedFn on_paired;          /* 可为 NULL */
    AmLogFn on_log;                /* 可为 NULL */
    void *user_data;
    AmFreeFn free_user_data;       /* 可为 NULL */
} AmClientCallbacks;

/* v3：生命周期策略。先用 am_lifecycle_init 填入默认值再修改；字段值按字面使用（0 就是 0）。 */
typedef struct AmLifecycle {
    AmLifecycleMode mode;            /* 默认 PERSISTENT */
    uint64_t idle_timeout_ms;        /* 默认 60000 */
    uint64_t hidden_idle_timeout_ms; /* 默认 15000；iOS 封装通常为 0 */
    uint64_t grace_ms;               /* 默认 10000 */
    AmResidency residency;           /* 默认 KEEP */
    AmWakeKind wake_kind;            /* 默认 AM_WAKE_UNSET：不上报唤醒描述 */
    const char *wake_target;         /* 可为 NULL：scheme / AUMID / bundle id / D-Bus 名 / 组件名 / URL */
    bool wake_background;            /* 能否不把窗口带到前台就唤醒 */
} AmLifecycle;

/* v3：am_client_new_ex 的扩展选项。struct_size 必须设为 sizeof(AmClientOptions)，
 * 以后追加字段时库据此判断调用方的版本；未知的更大值按已知部分处理。 */
typedef struct AmClientOptions {
    uint32_t struct_size;
    const AmLifecycle *lifecycle;    /* 可为 NULL：persistent */
    uint32_t connect_timeout_ms;     /* 0 表示默认值 5000 */
    AmIdleExitFn on_idle_exit;       /* 可为 NULL；user_data 为 AmClientCallbacks.user_data（callbacks 为 NULL 时为 NULL） */
    /* v7（4e 功耗，spec/lifecycle.md 第 11 节）：旧调用方的 struct_size 不含以下字段时取默认值。 */
    AmHeartbeatMode heartbeat;       /* 默认 AM_HEARTBEAT_AUTO：本地 IPC / 桌面本机回环不发心跳 */
    int32_t host_absent_retries;     /* idle / on-demand 下连续多少次"Host 不在"后转休眠；0 = 默认 3，负数 = 一直重连 */
    bool legacy_timers;              /* true：回退到 4e 之前的定时器行为（串行租约、无限重连、双向心跳） */
} AmClientOptions;

typedef struct AmToolSpec {
    const char *name;              /* 必填，[a-zA-Z0-9_.-]{1,64} */
    const char *description;       /* 必填 */
    const char *input_schema_json; /* 可为 NULL：无参数 */
    AmRisk risk;
    AmActivation activation;
    const char *title;             /* 可为 NULL */
    bool enabled;
} AmToolSpec;

typedef struct AmResourceSpec {
    const char *name;
    const char *description;
    const char *mime_type;         /* 可为 NULL：application/json */
} AmResourceSpec;

/* ---------------------------------------------------------------------------
 * 通用
 * ------------------------------------------------------------------------- */

/* 库版本，如 "0.1.0"。静态字符串。 */
const char *am_version(void);
/* 当前线程最近一次失败的错误说明；没有时返回空字符串。指针在当前线程下一次调用本库前有效。 */
const char *am_last_error_message(void);
void am_string_free(char *s);

/* ---------------------------------------------------------------------------
 * 客户端
 * ------------------------------------------------------------------------- */

/* 创建客户端并启动后台线程（不连接）。callbacks 可为 NULL。 */
AmStatus am_client_new(const AmClientConfig *config, const AmClientCallbacks *callbacks, AmClient **out);
/* 停止并释放客户端（阻塞到后台线程结束）。之后该客户端创建的其他句柄的操作返回 AM_ERR_STOPPED。 */
void am_client_free(AmClient *client);
AmStatus am_client_start(AmClient *client);
AmStatus am_client_stop(AmClient *client);
AmStatus am_client_set_visibility(AmClient *client, AmVisibility visibility, bool focused);
/* 当前状态；retry_in_ms、reason 可为 NULL。*reason 需用 am_string_free 释放（REJECTED / HOST_MISMATCH 时非 NULL；
 * v6 起 BACKOFF 有原因时也非 NULL；其他状态为 NULL）。 */
AmStatus am_client_state(const AmClient *client, AmStateStatus *status, uint64_t *retry_in_ms, char **reason);
/* 返回的字符串需 am_string_free。 */
char *am_client_instance_id(const AmClient *client);
/* 当前 token，没有时返回 NULL。需 am_string_free。 */
char *am_client_token(const AmClient *client);
/* v6：当前状态的错误码（spec/protocol.md 10.1）：REJECTED / HOST_MISMATCH 时总有，BACKOFF 时在连接失败等情况下有
 * （如 "HOST_NOT_RUNNING"、"IPC_PERMISSION_DENIED"），其他状态为 NULL。*code 需用 am_string_free 释放。 */
AmStatus am_client_state_code(const AmClient *client, char **code);
/* v6：Host 为当前连接分配的连接 ID（spec/protocol.md 10.3），未连接或 Host 未提供时为 NULL。*id 需用 am_string_free 释放。 */
AmStatus am_client_connection_id(const AmClient *client, char **id);

/* ---------------------------------------------------------------------------
 * 客户端：生命周期（v3，spec/lifecycle.md 第 8 节）
 * ------------------------------------------------------------------------- */

/* 用规范默认值填充 *lifecycle（mode = PERSISTENT、wake_kind = AM_WAKE_UNSET、wake_target = NULL）。 */
void am_lifecycle_init(AmLifecycle *lifecycle);
/* 同 am_client_new，另带扩展选项。options 可为 NULL（等同 am_client_new）；options->struct_size
 * 小于 v3 结构体大小时只读取其中已包含的字段。 */
AmStatus am_client_new_ex(const AmClientConfig *config, const AmClientCallbacks *callbacks,
                          const AmClientOptions *options, AmClient **out);
/* 处理操作系统激活参数 / URL（命令行、onOpenURL、D-Bus action 参数等），识别 app-mcp-wake:<token>、
 * <scheme>://app-mcp/wake?token=、#app-mcp-wake=<token>。不是本 SDK 的唤醒（或参数非法）返回 false。
 * 可以在 am_client_start 之前调用（冷启动唤醒）。 */
bool am_client_handle_wake(AmClient *client, const char *args);
/* App 主动回连（原因 app）。started 可为 NULL：写入是否因此发起了回连。 */
AmStatus am_client_wake(AmClient *client, bool *started);
AmStatus am_client_wake_with_reason(AmClient *client, AmWakeReason reason, bool *started);
/* on-demand 模式下主动连接；尚未 start 时等同于 start。started 可为 NULL。 */
AmStatus am_client_connect_now(AmClient *client, bool *started);
/* App 主动请求休眠（原因 app，不受空闲条件与持有影响）。changed 可为 NULL。 */
AmStatus am_client_sleep(AmClient *client, bool *changed);
AmStatus am_client_sleep_with_reason(AmClient *client, AmSleepReason reason, bool *changed);
/* 临时阻止自动休眠，直到 am_hold_release。 */
AmStatus am_client_hold(AmClient *client, AmHold **out);
/* 释放持有并释放句柄（hold 为 NULL 时无效果）。可在任意线程调用；客户端已释放时只释放句柄。 */
void am_hold_release(AmHold *hold);
/* 当前工具与资源定义的摘要（16 个十六进制字符）。需 am_string_free；出错返回 NULL。 */
char *am_client_tools_hash(const AmClient *client);
/* 从激活参数中提取唤醒令牌（不需要客户端，用于单实例转发前判断）。不是唤醒参数返回 NULL；
 * 返回值需 am_string_free。 */
char *am_parse_wake_token(const char *args);

/* 根作用域。返回的 scope 需 am_scope_free；对根作用域调用 am_scope_dispose 会注销全部工具与资源。 */
AmStatus am_client_root_scope(AmClient *client, AmScope **out);

/* ---------------------------------------------------------------------------
 * Scope
 * ------------------------------------------------------------------------- */

AmStatus am_scope_create(AmScope *parent, const char *name, AmScope **out);
/* 注销该 scope 下全部工具、资源与子 scope。幂等。 */
AmStatus am_scope_dispose(AmScope *scope);
void am_scope_free(AmScope *scope);

/* ---------------------------------------------------------------------------
 * 工具
 * ------------------------------------------------------------------------- */

AmStatus am_tool_register(AmScope *scope, const AmToolSpec *spec,
                          AmToolFn handler, void *user_data, AmFreeFn free_user_data,
                          AmTool **out);
/* 用新定义整体替换（spec->name 被忽略）。 */
AmStatus am_tool_update(AmTool *tool, const AmToolSpec *spec);
AmStatus am_tool_set_enabled(AmTool *tool, bool enabled);
/* 注销工具。幂等。 */
AmStatus am_tool_dispose(AmTool *tool);
void am_tool_free(AmTool *tool);

/* ---------------------------------------------------------------------------
 * 资源
 * ------------------------------------------------------------------------- */

AmStatus am_resource_register(AmScope *scope, const AmResourceSpec *spec,
                              AmReadFn reader, void *user_data, AmFreeFn free_user_data,
                              AmResource **out);
AmStatus am_resource_notify_changed(AmResource *resource);
AmStatus am_resource_dispose(AmResource *resource);
void am_resource_free(AmResource *resource);

/* ---------------------------------------------------------------------------
 * 调用
 * ------------------------------------------------------------------------- */

/* 以下三个字符串在 call 被消费前有效。 */
const char *am_call_id(const AmCall *call);
const char *am_call_tool_name(const AmCall *call);
const char *am_call_arguments_json(const AmCall *call);
bool am_call_is_cancelled(const AmCall *call);
/* 设置取消回调。已取消时立即在当前线程回调一次。 */
AmStatus am_call_set_cancel_callback(AmCall *call, AmCancelFn on_cancel, void *user_data, AmFreeFn free_user_data);
/* 成功完成并消费 call。data_json 可为 NULL（表示 null）。
 * 返回 AM_ERR_INVALID_JSON 时 call 不会被消费，可以重试。
 * 调用已被取消时仍然消费 call，并返回 AM_ERR_ALREADY_COMPLETED。
 * data_json 不是合法 UTF-8 时按 AM_ERR_INVALID_JSON 处理（不消费）；state_hints 含 NULL / 非法 UTF-8 时
 * 返回 AM_ERR_INVALID_ARGUMENT，调用以 HANDLER_ERROR 结束并消费 call。call 为 NULL 时返回 AM_ERR_INVALID_ARGUMENT。 */
AmStatus am_call_complete(AmCall *call, const char *data_json, const char *const *state_hints, size_t state_hints_len);
/* 失败完成并消费 call。kind 为协议错误类别字符串（如 "HANDLER_ERROR"），未知值按 HANDLER_ERROR 处理。 */
AmStatus am_call_fail(AmCall *call, const char *kind, const char *message);
/* v3：失败完成并消费 call，附带结构化详情（JSON 文本；对象的字段合并进错误的 data，其他值放在
 * data.details）。details_json 为 NULL 等同 am_call_fail。details_json 非法时返回 AM_ERR_INVALID_JSON
 * 且不消费 call（可以重试）。 */
AmStatus am_call_fail_with_details(AmCall *call, const char *kind, const char *message, const char *details_json);
/* v3：延长持有：调用完成后仍阻止自动休眠（handler 发起的长任务），直到 am_hold_release。
 * 不消费 call；调用已结束时返回 AM_ERR_ALREADY_COMPLETED。 */
AmStatus am_call_hold(const AmCall *call, AmHold **out);

/* ---------------------------------------------------------------------------
 * 资源读取
 * ------------------------------------------------------------------------- */

const char *am_read_resource_name(const AmRead *read);
/* 成功完成并消费 read。返回 AM_ERR_INVALID_JSON 时不消费。 */
AmStatus am_read_complete(AmRead *read, const char *contents_json);
AmStatus am_read_fail(AmRead *read, const char *kind, const char *message);

#ifdef __cplusplus
}
#endif

#endif /* APP_MCP_H */
