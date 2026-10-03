/*
 * app_mcp_hub.h —— app-mcp Hub SDK（Agent 端）的 C 接口（契约）。
 *
 * 由 bindings/hub-c（Rust cdylib / staticlib，库名 app_mcp_hub）实现，是 C、C++、C#（P/Invoke）、
 * Dart（dart:ffi）等语言的 Hub 绑定的共同基础。语义与 crates/hub（spec/hub-api.md）一致。
 *
 * 约定
 * - 所有导出符号以 am_hub_ / AmHub / AM_HUB_ 为前缀，可与 app_mcp.h（App 端）的库同时链接进一个进程。
 * - 所有字符串为 UTF-8、以 NUL 结尾。复杂结构一律以 JSON 字符串传递（字段为 camelCase，
 *   与 crates/hub 的 serde 形式一致，见 spec/hub-api.md 3.1、3.4）。
 * - 传入的字符串由调用方持有，函数返回后即可释放（库会复制）。
 * - 库返回给调用方的 char*（out 参数、回调参数）归接收方所有，用 am_hub_string_free 释放
 *   （可以在回调返回后、任意线程释放）。const char* 返回值归库所有。
 * - 函数返回 AmHubStatus；非 AM_HUB_OK 时可立即用 am_hub_last_error_message 取得说明（线程局部）。
 *   失败时 out 参数被置为 NULL。
 * - 所有函数线程安全（am_hub_free 除外：调用后不得再使用该句柄）。Rust panic 不会跨越 FFI 边界，
 *   转换为 AM_HUB_ERR_PANIC。
 * - 回调在 Hub 的分发线程上串行执行（不在调用方线程，也不在 Hub 的 tokio 工作线程），不持有内部锁，
 *   应尽快返回；可以在回调中调用本库的任何函数（包括 am_hub_approval_complete），但不要在回调中
 *   阻塞等待另一个回调。
 * - 一次性回调（AmHubResultFn）：发起函数返回 AM_HUB_OK 时，回调恰好调用一次（Hub 停止时以
 *   "CANCELLED" 错误结果调用）；返回其他状态时不调用。user_data 由调用方自行管理。
 * - 常驻回调（事件、审批、配对、唤醒）：user_data 与 free_user_data 传入后归库所有，库在不再使用时
 *   （被替换、清除或 am_hub_free）调用 free_user_data（可为 NULL；可能在任意线程调用）。
 *
 * 版本
 * - v1：初版。
 * - v2（生命周期，spec/hub-api.md 3.5）：只做新增，v1 的函数签名与语义不变。
 *   · 类型 AmHubWake、AmHubWakerFn；函数 am_hub_set_waker_cb、am_hub_waker_complete（自定义唤醒，
 *     如 Android 厂商发送显式广播）。
 *   · am_hub_start 配置新增可选字段 leaseTtlMs、wakeTimeoutMs、wakeTokenTtlMs、dormantTtlMs、
 *     dormantReplacedByNewInstance、wakeFromLaunch。
 *   · JSON 中新增：HubTool.availability 取值 "dormant"；AppInfo.dormantInstances（InstanceInfo 数组）；
 *     事件 {"type":"appDormant","appId","instanceId"} 与 {"type":"appWaking","appId","instanceId"|null}。
 * - v3（渐进暴露，spec/hub-api.md 3.7）：只做新增。
 *   · am_hub_start 配置新增可选字段 toolExposure、toolExposureThreshold、waker。
 *   · ToolFilter 新增可选字段 session（渐进暴露按会话计算）；新内置工具 apps.tools。
 * - v4（本地 IPC 传输，spec/protocol.md 1.2）：只做新增。
 *   · am_hub_start 配置新增可选字段 ipcEndpoint（缺省监听平台默认 IPC 端点）；函数 am_hub_ipc_endpoint。
 *   · JSON 中新增：InstanceInfo.pid（经本地 IPC 连接的实例进程号，缺省表示未知）。
 * - v5（合并端口与单实例，spec/protocol.md 1.3–1.7）：不兼容变更，AM_HUB_API_VERSION 升为 3。
 *   · am_hub_ws_addr 改名 am_hub_listen_addr；配置字段 wsAddr 改为 listen（同一端口承载 /app、/healthz，
 *     mcpHttp 时另有 /mcp），旧名报 AM_HUB_ERR_INVALID_JSON。
 *   · am_hub_start 配置新增可选字段 mcpHttp、runDir。
 *   · am_hub_serve_http 的额外监听器与主服务路由相同（/app、/mcp、/healthz）。
 * - v6（诊断，spec/hub-api.md 3.9）：只做新增，AM_HUB_API_VERSION 仍为 3。
 *   · 函数 am_hub_status_json（HubStatus，与 GET /status 相同）。
 *   · JSON 中新增：InstanceInfo.connectionId（Hub 分配的连接 ID，与日志 cid 相同；休眠实例缺省）；
 *     事件 {"type":"appDiagnostic","appId","instanceId","code","message","count"}（SDK 上报的连接问题）。
 * - v7（功耗，spec/lifecycle.md 第 11–12 节）：只做新增，AM_HUB_API_VERSION 仍为 3。
 *   · am_hub_start 配置新增可选字段 wakeRateLimit、legacyHeartbeat。
 *   · HubStatus JSON 中新增：AppStatus.wakes；InstanceStatus.power（reconnects、wakes、onlineSecs、heartbeats、
 *     heartbeatMs、lifecycleMode、awakeReasons）。
 * - v8（自适应租约与按需在线，spec/lifecycle.md 第 13 节）：只做新增，AM_HUB_API_VERSION 仍为 3。
 *   · am_hub_start 配置新增可选字段 lease（{"adaptive","window","marginMs","minMs","maxMs","idleRevokeMs"}）。
 *   · HubStatus JSON 中新增：lease（mode、defaultMs、minMs、maxMs、marginMs、window、idleRevokeMs、adaptiveGrants、
 *     defaultGrants、revokedSessionEnd、revokedIdle、pairs[{session、appId、samples、nextTtlMs、adaptive}]）。
 *   · awakeReasons 的 "subscription" 只计声明 realtime 的资源的订阅；ResourceInfo 可带 realtime。
 * - v9（资源保护与结果约定，spec/hub-api.md 3.11、spec/protocol.md 3.2）：只做新增，AM_HUB_API_VERSION 仍为 3。
 *   · am_hub_start 配置新增可选字段 limits（{"toolRatePerMinute","toolRateBurst","appRatePerMinute","appRateBurst",
 *     "maxArgumentsBytes","maxResultBytes","maxResourceBytes"}）与 outputValidation（"off" | "log" | "reject"）。
 *   · JSON 中新增：HubTool.annotations（Agent 实际看到的 MCP 工具注解）、HubTool.outputSchema；HubResource.annotations；
 *     CallOutcome.status / stateResource / summary / annotations；ApprovalRequest.annotations；
 *     HubStatus.limits、HubStatus.outputValidation；AppStatus.rateLimited、tooLarge、tools（ToolDeclaration 数组）。
 *   · 错误类别新增 "RATE_LIMITED"（details：retryAfterMs、scope、perMinute、burst、appId、tool）与
 *     "PAYLOAD_TOO_LARGE"（details：part、sizeBytes、limitBytes）。
 * - v10（策略挂点，spec/hub-api.md 3.13）：只做新增，AM_HUB_API_VERSION 仍为 3。
 *   · am_hub_start 配置新增可选字段 policy（{"rules":[{"id","action":"hide"|"deny","app","tool"?,"annotations"?,
 *     "hooks"?}]}）；函数 am_hub_set_policy（运行中替换规则集）。
 *   · HubStatus JSON 中新增：policy（rules[{规则字段…, hits}]、loadedAtMs、lastError?{message, atMs}）。
 *   · 错误类别新增 "POLICY_DENIED"（details：ruleId、hook、appId、tool）与 "USER_ACTION_REQUIRED"
 *     （details：reason?、uri?，需要用户本人操作后才能继续）。被 hide 的工具调用为 "TOOL_NOT_FOUND"。
 * - v11（调用进度，spec/hub-api.md 3.12）：只做新增，AM_HUB_API_VERSION 仍为 3。
 *   · AmHubProgressFn + am_hub_call_with_progress：调用时接收 App 报告的进度。
 *   · am_hub_start 配置新增可选字段 progressIntervalMs（进度转发的最小间隔，缺省 250）。
 * - v12（休眠记录持久化，spec/hub-api.md 3.5「持久化」）：只做新增，AM_HUB_API_VERSION 仍为 3。
 *   · am_hub_start 配置新增可选字段 stateDir。
 *   · HubStatus JSON 中新增：dormantStore（dir、loadedInstances、expiredInstances、writes、issues[{file, reason}]、
 *     lastError?；未配置 stateDir 时缺省）。
 * - v13（页面导航与 Agent 显式控制，spec/hub-api.md 3.14 / 3.15）：只做新增，AM_HUB_API_VERSION 仍为 3。
 *   · am_hub_start 配置新增可选字段 navigateTimeoutMs。
 *   · CallRequest 新增可选字段 idempotencyKey（1..=256 个字符，原样转交 App）。
 *   · JSON 中新增：HubTool.surface（"app" | "view"）/ page（缺省表示无）；CallOutcome.routedTo（改调后台替代时）。
 *   · 新内置工具 apps.activate、apps.release（总是列出），apps.page、apps.navigate（有页面目录时）。
 * - v14（调用元信息，docs/plans/19-result-contract.md R4）：只做新增，AM_HUB_API_VERSION 仍为 3。
 *   · JSON 中新增：CallOutcome.durationMs（Hub 收到调用到得出结果的毫秒数）、CallOutcome.woke（本次 App 工具调用是否
 *     经历了唤醒；内置 / 上游工具恒为 false）。
 * - v15（无会话 MCP 请求与 Agent 任务，spec/hub-api.md 3.3 / 3.6 / 3.7 / 3.9）：只做新增，AM_HUB_API_VERSION 仍为 3。
 *   · am_hub_start 配置新增可选字段 taskIdleTtlMs、statelessToolExposure、principalSelectTtlMs、statelessListTtlMs。
 *   · JSON 中新增：ApprovalRequest.principal / clientName（只在 MCP 出口发起的审批中出现）；
 *     HubStatus.tasks（Agent 任务数组）。
 * - v16（MCP 2026-07-28 与 subscriptions/listen，spec/hub-api.md 3.6「协议版本」「通知」）：只做新增，AM_HUB_API_VERSION 仍为 3。
 *   · am_hub_start 配置新增可选字段 mcpProtocolMode、maxListenStreams、maxListenResources。
 *   · JSON 中新增：HubStatus.mcpListenStreams（进行中的 subscriptions/listen 流数）。
 * - v17（任务句柄，spec/hub-api.md 3.6「任务句柄」）：只做新增，AM_HUB_API_VERSION 仍为 3。
 *   · am_hub_start 配置新增可选字段 maxTaskHandles。
 * - v18（按 Agent 的策略与记账，第 16 项 P2 / P3，spec/hub-api.md 3.11 / 3.13）：只做新增，AM_HUB_API_VERSION 仍为 3。
 *   · limits 新增可选字段 agentRatePerMinute / agentRateBurst（每个已登记 Agent 一级限流，缺省不限）；
 *     policy 规则新增可选字段 agent（只用于 deny）。
 *   · JSON 中新增：HubStatus.agents（已登记的 Agent 名）、HubStatus.usage（按调用方记账）、tasks[].agent。
 * - v19（嵌入式 Hub 的 Agent 登记，spec/hub-api.md 3.6「Agent 身份」）：只做新增，AM_HUB_API_VERSION 仍为 3。
 *   · am_hub_start 配置新增可选字段 agents（[{"name","token"}]）；函数 am_hub_set_agents（运行中替换登记）。
 */
#ifndef APP_MCP_HUB_H
#define APP_MCP_HUB_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* 3：v5 合并端口（am_hub_listen_addr、配置 listen），见文件头“版本”。 */
#define AM_HUB_API_VERSION 3

/* ---------------------------------------------------------------------------
 * 状态码与枚举
 * ------------------------------------------------------------------------- */

typedef enum AmHubStatus {
    AM_HUB_OK = 0,
    AM_HUB_ERR_INVALID_ARGUMENT = 1,   /* 空指针、非法 UTF-8、非法枚举值 */
    AM_HUB_ERR_INVALID_JSON = 2,       /* JSON 无法解析或字段不符 */
    AM_HUB_ERR_INVALID_CONFIG = 3,     /* 配置无效（如清单无法加载） */
    AM_HUB_ERR_IO = 4,                 /* 监听地址无法绑定等 */
    AM_HUB_ERR_HUB = 5,                /* Hub 拒绝了操作；错误说明为 "<KIND>: <message>" */
    AM_HUB_ERR_ALREADY_COMPLETED = 6,  /* 审批 / 配对已超时或被取消（句柄仍被消费） */
    AM_HUB_ERR_STOPPED = 7,            /* Hub 已停止 */
    AM_HUB_ERR_INTERNAL = 8,
    AM_HUB_ERR_PANIC = 9,
    /* 本构建未包含所需能力（cargo feature，spec/hub-api.md 3.10）：mcpHttp / upstreams / am_hub_serve_http；
     * 错误说明含缺少的 feature 名。v3 之后新增（API 版本号不变），此前同类错误报 AM_HUB_ERR_IO。 */
    AM_HUB_ERR_UNSUPPORTED = 10
} AmHubStatus;

/* 工具格式（spec/hub-api.md 第 5 节）。 */
typedef enum AmHubToolFormat {
    AM_HUB_FORMAT_MCP = 0,
    AM_HUB_FORMAT_OPENAI_CHAT = 1,
    AM_HUB_FORMAT_OPENAI_RESPONSES = 2,
    AM_HUB_FORMAT_ANTHROPIC = 3,
    AM_HUB_FORMAT_GEMINI = 4
} AmHubToolFormat;

/* ---------------------------------------------------------------------------
 * 不透明句柄
 * ------------------------------------------------------------------------- */

typedef struct AmHub AmHub;
typedef struct AmHubApproval AmHubApproval;  /* 一次审批；由 am_hub_approval_complete 消费 */
typedef struct AmHubPairing AmHubPairing;    /* 一次配对；由 am_hub_pairing_complete 消费 */
typedef struct AmHubWake AmHubWake;          /* v2：一次唤醒；由 am_hub_waker_complete 消费 */

/* ---------------------------------------------------------------------------
 * 回调（所有 char* 参数归回调方所有，用 am_hub_string_free 释放）
 * ------------------------------------------------------------------------- */

typedef void (*AmHubFreeFn)(void *user_data);

/* 一次性结果。result_json 非 NULL，形状见各发起函数。 */
typedef void (*AmHubResultFn)(void *user_data, char *result_json);

/* Hub 事件。event_json 为 {"type":"appConnected","appId":…,"instanceId":…} 等（spec/hub-api.md 3.1）；
 * 回调处理过慢导致事件丢失时收到 {"type":"lagged","skipped":<n>}，此时应重新查询全量状态。 */
typedef void (*AmHubEventFn)(void *user_data, char *event_json);

/* 调用审批。request_json 为 ApprovalRequest（callId、appId、appName、tool、title、description、risk、
 * arguments、session；v9 起另有 annotations：与 HubTool.annotations 相同；v15 起 MCP 出口发起的审批另有 principal：
 * 认证主体（取自传输层凭据，现在恒为 "local"）与 clientName：客户端自报的 clientInfo.name，不可信、仅供显示，不得据此授权；
 * 经 am_hub_call 发起的审批不含这两个字段；session 在 MCP 出口为调用方键 mcp:<n> / principal:<主体>）。approval 的所有权转移给回调方，必须最终调用 am_hub_approval_complete 恰好一次
 * （可在任意线程、回调返回之后）。超时（approval.timeout / responseTimeout）或调用被取消后完成返回
 * AM_HUB_ERR_ALREADY_COMPLETED。 */
typedef void (*AmHubApprovalFn)(void *user_data, char *request_json, AmHubApproval *approval);

/* App 配对。request_json 为 PairingRequest（appId、appName、origin、clientKind、instanceId）。
 * pairing 的所有权转移给回调方，必须最终调用 am_hub_pairing_complete 恰好一次。 */
typedef void (*AmHubPairingFn)(void *user_data, char *request_json, AmHubPairing *pairing);

/* v2：唤醒 App。request_json 为 WakeRequest：
 *   {"appId", "instanceId": <休眠实例 ID>|null（null = 冷启动）,
 *    "descriptor": {"kind": "uri"|"aumid"|"apple-event"|"dbus"|"android-intent"|"web-url", "target"?, "background"},
 *    "token": <32 位十六进制一次性令牌>, "activationArg": "app-mcp-wake:<token>"}
 * 回调方按描述激活 App（如 Android 发送显式广播，把 activationArg / token 交给 App 的 handleWake），
 * 然后调用 am_hub_waker_complete 恰好一次（可在任意线程、回调返回之后）：ok = true 表示已发出激活，
 * Hub 随后等待 App 回连（wakeTimeoutMs）。wake 的所有权转移给回调方。 */
typedef void (*AmHubWakerFn)(void *user_data, char *request_json, AmHubWake *wake);

/* ---------------------------------------------------------------------------
 * 通用
 * ------------------------------------------------------------------------- */

/* 库版本，如 "0.1.0"。静态字符串。 */
const char *am_hub_version(void);
/* 当前线程最近一次失败的错误说明；没有时返回空字符串。指针在当前线程下一次调用本库前有效。 */
const char *am_hub_last_error_message(void);
/* 释放库分配的字符串。NULL 忽略。 */
void am_hub_string_free(char *s);

/* ---------------------------------------------------------------------------
 * 生命周期
 * ------------------------------------------------------------------------- */

/* 创建 tokio 运行时与分发线程并启动 Hub。config_json 可为 NULL（全部默认）。字段（均可省略）：
 *   listen               v5：HTTP 监听地址（/app 为 App 的 WebSocket 连接，另有 /healthz）。省略时为
 *                        "127.0.0.1:7717"，被占用时依次尝试 7737、7757；显式给出时只绑定该地址；端口 0 随机；
 *                        null = 不开。非回环地址报 AM_HUB_ERR_IO
 *   mcpHttp              v5：是否在 listen 上提供 MCP Streamable HTTP（/mcp），默认 false
 *   runDir               v5：单实例锁与登记文件目录（<runDir>/hub.lock、endpoints.json）；省略时不参与。
 *                        锁已被其他 Hub 持有时报 AM_HUB_ERR_IO
 *   stateDir             v12：持久状态目录：休眠记录写到 <stateDir>/dormant/<appId>.json（原子写、仅当前用户可读），
 *                        启动时读回，重启前休眠的 App 仍可列出、可唤醒。省略时不读写任何文件
 *   ipcEndpoint          v4：本地 IPC 端点（原生 App 默认连接这里，spec/protocol.md 1.2）："unix:<绝对路径>" /
 *                        "pipe:\\\\.\\pipe\\<名称>"（JSON 转义）；缺省为平台默认端点；null = 不开。
 *                        已有 Hub 在该端点监听时报 AM_HUB_ERR_IO
 *   manifests            静态清单对象数组（spec/manifest.md）
 *   manifestFiles        静态清单文件路径数组
 *   manifestDir          清单目录（不存在时忽略）
 *   allowOrigins         额外允许的 Origin 模式数组
 *   pingIntervalMs / idleTimeoutMs / hiddenIdleTimeoutMs / invokeTimeoutMs / responseTimeoutMs /
 *   listChangedDebounceMs / pairingTimeoutMs
 *   progressIntervalMs   v11：进度转发的最小间隔（间隔内只保留最新一条），缺省 250
 *   upstreams            {"<name>": {"command":…, "args":[…], "env":{…}}}
 *   approval             {"requireAtOrAbove": "destructive", "timeout": <ms>}（默认不审批）
 *   —— v2 生命周期（spec/hub-api.md 3.5）——
 *   leaseTtlMs           调用完成后发给实例的租约时长，默认 60000；0 关闭
 *   wakeTimeoutMs        唤醒后等待 App 回连的上限，默认 15000（超时 → APP_NOT_RESPONDING）
 *   navigateTimeoutMs    v13：导航等待上限（App 回复 + 目标工具注册，spec/hub-api.md 3.14 / 3.15），默认 5000，
 *                        独立于 wakeTimeoutMs（超时 → NAVIGATION_FAILED，reason "timeout" / "tool-not-registered"）
 *   wakeTokenTtlMs       唤醒令牌有效期，默认 60000
 *   dormantTtlMs         休眠记录保留时长，默认 86400000（24 小时）
 *   dormantReplacedByNewInstance  同一 appId 以新实例 ID 连接时移除其休眠记录，默认 true
 *   wakeFromLaunch       App 未运行且清单无显式 wake 时由 launch 推导唤醒方式，默认 false
 *   waker                "system"（默认）/ "none"（不唤醒）/ {"exec": ["程序", "参数", …]}（spec/hub-api.md 3.5）；
 *                        am_hub_set_waker_cb 设置的回调优先
 *   wakeRateLimit        每 App 每分钟最多唤醒次数，默认 6；0 不限（超出 → LAUNCH_FAILED，data.code = WAKE_RATE_LIMITED）
 *   legacyHeartbeat      回退到旧心跳（对所有连接发 ping 并按无消息断开），默认 false
 *   lease                v8：自适应租约 {"adaptive": true, "window": 20, "marginMs": 5000, "minMs": 5000, "maxMs": 60000,
 *                        "idleRevokeMs": 30000}（缺省字段取这些默认值）；租约 = 同一（会话, App）最近 window 个调用间隔的
 *                        p90 + marginMs，限制在 [minMs, maxMs]，样本不足 3 个时用 leaseTtlMs；adaptive=false 回退到固定
 *                        leaseTtlMs；idleRevokeMs：会话无请求这么久后收回其默认租约（0 不收回）。window=0 或 minMs>maxMs
 *                        报 AM_HUB_ERR_INVALID_CONFIG
 *   —— v9 资源保护与结果约定（spec/hub-api.md 3.11）——
 *   limits               {"toolRatePerMinute": 120, "toolRateBurst": 30, "appRatePerMinute": 600, "appRateBurst": 60,
 *                        "maxArgumentsBytes": 1048576, "maxResultBytes": 4194304, "maxResourceBytes": 4194304}
 *                        （缺省字段取这些默认值）。限流按（App, 工具）与按 App 两级令牌桶，超出 → RATE_LIMITED；
 *                        参数 / 结果 / 资源超过字节上限 → PAYLOAD_TOO_LARGE（不截断）。*PerMinute = 0 不限流，
 *                        *Bytes = 0 不限大小；限流时 *Burst = 0 报 AM_HUB_ERR_INVALID_CONFIG；未知字段报 AM_HUB_ERR_INVALID_JSON
 *   outputValidation     结果与工具 outputSchema 不符时："off" 不校验 / "log"（默认）只记日志 / "reject" 以 HANDLER_ERROR 结束
 *   —— v10 策略挂点（spec/hub-api.md 3.13）——
 *   policy               {"rules": [{"id": "no-clear", "action": "hide"|"deny", "app": "shop"（或前缀 "shop*" / "*"）,
 *                        "tool"?: "cart.clear"（局部名，可带末尾 *）, "annotations"?: {"destructiveHint": true, …},
 *                        "hooks"?: ["call", "wake"]（只用于 deny，缺省 ["call"]）}]}。hide：工具（或整个 App）不出现在
 *                        任何列表中，调用为 TOOL_NOT_FOUND；deny：调用 / 唤醒以 POLICY_DENIED 结束（details.ruleId）。
 *                        缺省无规则（行为不变）；规则不合法报 AM_HUB_ERR_INVALID_CONFIG，未知字段报 AM_HUB_ERR_INVALID_JSON
 *   —— v3 渐进暴露（spec/hub-api.md 3.7）——
 *   toolExposure         "auto"（默认，App 工具总数超过阈值时渐进）/ "progressive" / "all"
 *   toolExposureThreshold  auto 的阈值，默认 40
 *   —— v15 无会话 MCP 请求（spec/hub-api.md 3.6 / 3.7）——
 *   taskIdleTtlMs        无会话调用方（principal:<主体>）的 Agent 任务在请求流空闲多久后回收（收回其租约、清除选择），
 *                        默认 600000；0 不因空闲回收
 *   statelessToolExposure  无会话请求的工具暴露方式："all"（默认）/ "progressive" / "auto"（阈值同 toolExposureThreshold）；
 *                        渐进时列表只含内置工具与全局选定实例的 App，不随调用变化
 *   principalSelectTtlMs 无会话请求的主体级 apps.select 选择的空闲有效期，默认 60000；0 不单独过期
 *   statelessListTtlMs   无会话请求的列表结果所带缓存提示 ttlMs，默认 5000
 *   —— v16 MCP 出口协议版本与通知（spec/hub-api.md 3.6）——
 *   mcpProtocolMode      "auto"（默认：initialize 客户端走 legacy 会话，每请求自带 _meta 的客户端可协商 2026-07-28）/
 *                        "legacyOnly"（回退开关：只声明到 2025-11-25，subscriptions/listen 不可用）
 *   maxListenStreams     每个主体同时打开的 subscriptions/listen 流数上限，默认 16；0 不提供 listen
 *   maxListenResources   一个 listen 流接受的资源 URI 数上限，默认 256
 *   —— v17 任务句柄（spec/hub-api.md 3.6「任务句柄」）——
 *   maxTaskHandles       每个主体同时存在的任务句柄数上限，默认 32（超出时 apps.task.begin 报 RATE_LIMITED）；
 *                        0 不提供任务句柄（apps.task.* 不列出，taskId 一律无效）
 *   —— v19 Agent 身份（spec/hub-api.md 3.6「Agent 身份」）——
 *   agents               [{"name","token"}]：按 Agent 发的访问令牌，/mcp 出示时请求主体为 agent:<name>（只区分与归属，
 *                        不做授权）；名字 1–64 个字母、数字、-、_、.，令牌 32–512 个可见 ASCII 字符；缺省空
 *   workerThreads        tokio 工作线程数（默认 2）
 * 未知字段报 AM_HUB_ERR_INVALID_JSON。清单无效报 AM_HUB_ERR_INVALID_CONFIG；地址无法绑定报 AM_HUB_ERR_IO。 */
AmHubStatus am_hub_start(const char *config_json, AmHub **out_hub);

/* 停止 Hub（阻塞）：进行中的调用以 CANCELLED 结束，关闭所有 App 连接与上游子进程。幂等。
 * 之后除 am_hub_free 外的操作返回 AM_HUB_ERR_STOPPED。 */
void am_hub_shutdown(AmHub *hub);

/* 停止（若尚未停止）并释放 Hub：等待分发线程处理完已排队的回调，然后调用各 free_user_data。
 * 未完成的审批 / 配对按拒绝处理。可以在回调中调用（此时不等待分发线程）。NULL 忽略。 */
void am_hub_free(AmHub *hub);

/* v5：HTTP 服务（/app、/healthz[、/mcp]）实际监听的地址（如 "127.0.0.1:52341"，App 端点为
 * "ws://<地址>/app"）；未开启或已停止时返回 NULL。需 am_hub_string_free。 */
char *am_hub_listen_addr(const AmHub *hub);

/* v4：本地 IPC 连接服务的端点（如 "unix:/run/user/1000/app-mcp/hub.sock"，可直接作为原生 SDK 的 host_url）；
 * 未开启或已停止时返回 NULL。需 am_hub_string_free。 */
char *am_hub_ipc_endpoint(const AmHub *hub);

/* 额外启动一个 HTTP 监听器（路由与主服务相同：/app、/healthz，且总是提供 /mcp）。非回环地址需要 allow_remote。
 * out_addr 可为 NULL；否则写入实际监听地址（需 am_hub_string_free）。 */
AmHubStatus am_hub_serve_http(AmHub *hub, const char *addr, bool allow_remote, char **out_addr);

/* ---------------------------------------------------------------------------
 * 查询（同步，读快照；*out_json 需 am_hub_string_free）
 * ------------------------------------------------------------------------- */

/* AppInfo 数组（含上游，kind = "upstream"）。 */
AmHubStatus am_hub_apps_json(const AmHub *hub, char **out_json);
/* HubTool 数组（v9 起含 annotations：声明优先、缺少的按 risk 推导；outputSchema?：App 声明的原样 schema；
 * v13 起 App 工具含 surface?："app" | "view"（界面依赖）与 page?：所在页面，内置 / 上游工具缺省）。filter_json 可为 NULL：ToolFilter {apps, maxRisk, onlyAvailable, includeBuiltin, session}。
 * 渐进暴露生效且未给 apps 时，只含内置工具与 session 会话已展开 / 调用过 / 选定了实例的 App 的工具。 */
AmHubStatus am_hub_tools_json(const AmHub *hub, const char *filter_json, char **out_json);
/* HubResource 数组（v9 起可带 annotations：MCP 内容注解）。 */
AmHubStatus am_hub_resources_json(const AmHub *hub, char **out_json);
/* AppOverviewInfo；App 未知或没有总览时为 "null"。 */
AmHubStatus am_hub_overview_json(const AmHub *hub, const char *app_id, char **out_json);
/* HubStatus（v6，与 GET /status 相同）：
 *   {service, version, user?, pid, listen?, ipcEndpoint?, startedAtMs, mcpHttp,
 *    auth: {tokenConfigured, tokenRequiredWithoutOrigin}, mcpSessions,
 *    apps: [{appId, name, kind, state: "connected"|"waking"|"dormant"|"disconnected",
 *            instances: [InstanceInfo + state: "connected"|"dormant"|"waking"],
 *            lastError?: {code?, message, atMs}}],          按 appId 排序，含上游
 *    reports: [{appId, instanceId, connectionId, code, message, count, receivedAtMs}]}  最近的 SDK 上报，旧的在前
 * v7 / v8 起另有 AppStatus.wakes、InstanceStatus.power、lease（见文件头“版本”）。v9 起另有：
 *   limits（同配置 limits，全部字段给出）、outputValidation；
 *   AppStatus.rateLimited / tooLarge（启动以来 RATE_LIMITED / PAYLOAD_TOO_LARGE 的拒绝次数）、
 *   AppStatus.tools：[{name（局部名）, risk, annotations?（App 声明的原样注解）, effective（Agent 看到的注解）, outputSchema（bool：是否声明）}]
 * v10 起另有 policy：{rules: [{id, action, app, tool?, annotations?, hooks?, hits（自本规则集生效以来的拒绝 / 按不存在处理次数）}],
 *   loadedAtMs, lastError?: {message, atMs}（最近一次 am_hub_set_policy 失败，之后成功时清除）}
 * v12 起另有 dormantStore（配置了 stateDir 时）：{dir（<stateDir>/dormant）, loadedInstances（启动时读回的实例数）,
 *   expiredInstances（启动时因过期丢弃的实例数）, writes（启动以来成功写入 / 删除文件的次数）,
 *   issues: [{file, reason}]（启动时跳过的文件：损坏、版本未知、超出上限）, lastError?（最近一次写入失败）}
 * v15 起另有 tasks：Agent 任务（spec/hub-api.md 3.6），按 caller 排序：[{id（"task-<128 位十六进制>"）,
 *   caller（调用方键 "mcp:<n>" | "principal:<主体>" | "api" | "api:<session>"）, kind（"mcpSession" | "principal" | "api"）,
 *   selections: [{appId, instanceId, expiresInMs?（主体级选择距失效的毫秒数）}], leases: [{connectionId, expiresInMs}],
 *   inflight（进行中的请求数）, idleMs?（距最近一次请求活动的毫秒数）}]
 * v16 起另有 mcpListenStreams：进行中的 subscriptions/listen 流数（mcpSessions 只计 legacy 会话）。 */
AmHubStatus am_hub_status_json(const AmHub *hub, char **out_json);

/* ---------------------------------------------------------------------------
 * 调用
 * ------------------------------------------------------------------------- */

/* 异步调用工具。request_json 为 CallRequest：{name, arguments, instanceId, timeout(ms), callId, session,
 * idempotencyKey（v13：Agent 幂等键，1..=256 个字符，原样转交 App；不合法 → INVALID_INPUT）}。
 * out_call_id 可为 NULL；否则写入本次 callId（请求未给出时自动生成，需 am_hub_string_free），供 am_hub_cancel_call。
 * cb 收到 CallOutcome JSON：
 *   {"callId":…, "result": {"ok": <data>} | {"error": {"kind","message","details"?}},
 *    "stateHints": […], "instanceId": …|null, "overview": AppOverviewInfo|null,
 *    v9："status": "done"|"pending"|"partial"|"noop", "stateResource"?: "app-mcp://<appId>/<名>",
 *        "summary"?: …, "annotations"?: {"audience"?, "priority"?, "lastModified"?},
 *    v13："routedTo"?: 改调后台替代时实际调用的工具全名（spec/hub-api.md 3.14），
 *    v14："durationMs": <毫秒>, "woke": <bool>（本次 App 工具调用是否经历了唤醒）}
 * 名称无法解析（appId 未知等）也以 CallOutcome 形式返回（result.error，kind 为 TOOL_NOT_FOUND）。 */
AmHubStatus am_hub_call(AmHub *hub, const char *request_json, AmHubResultFn cb, void *user_data,
                        char **out_call_id);
/* v11：调用进度。progress_json 为 {"callId":…, "progress": <number>, "total"?: <number>, "message"?: <string>}
 * （App 报告、经 Hub 按 progressIntervalMs 合并且递增；message 最长 200 字符）。 */
typedef void (*AmHubProgressFn)(void *user_data, char *progress_json);
/* v11：同 am_hub_call，并接收进度。on_progress 可为 NULL（等同 am_hub_call）；与 cb 共用 user_data。
 * 进度回调与结果回调在同一分发线程上串行执行：全部进度回调先于 cb，cb 之后不再有进度回调（user_data 可在 cb 中释放）。
 * 进度不保证送达（未连接、合并、Hub 释放中时丢弃）。 */
AmHubStatus am_hub_call_with_progress(AmHub *hub, const char *request_json, AmHubResultFn cb,
                                      AmHubProgressFn on_progress, void *user_data, char **out_call_id);
/* 取消进行中的调用（含等待审批中的）；未知 callId 忽略。 */
AmHubStatus am_hub_cancel_call(AmHub *hub, const char *call_id);

/* 异步读取资源（app-mcp://<appId>/<name>）。cb 收到
 *   {"ok": {"uri","mimeType","text","blob"}} 或 {"error": {"kind","message","details"?}}。 */
AmHubStatus am_hub_read_resource(AmHub *hub, const char *uri, AmHubResultFn cb, void *user_data);

/* 订阅资源变化，之后收到 {"type":"resourceUpdated","uri":…} 事件。失败为 AM_HUB_ERR_HUB。 */
AmHubStatus am_hub_subscribe(AmHub *hub, const char *uri);
AmHubStatus am_hub_unsubscribe(AmHub *hub, const char *uri);

/* 指定某 App 的目标实例（全局默认）。instance_id 为 NULL 时清除。 */
AmHubStatus am_hub_select_instance(AmHub *hub, const char *app_id, const char *instance_id);
/* 清除某会话的状态（已附带的总览、apps.select）。session 为 NULL 表示默认会话。 */
AmHubStatus am_hub_reset_session(AmHub *hub, const char *session);
/* v10：替换策略规则集（JSON 形式同配置 policy；"{}" 或 {"rules":[]} 清空），命中计数清零。
 * 不是合法 JSON 或有未知字段 → AM_HUB_ERR_INVALID_JSON；规则不合法 → AM_HUB_ERR_INVALID_CONFIG（之前的规则继续生效，
 * 原因记入 am_hub_status_json 的 policy.lastError）。 */
AmHubStatus am_hub_set_policy(AmHub *hub, const char *policy_json);
/* v19：替换 Agent 登记（JSON 形式同配置 agents；"[]" 清空），只影响之后到达的 MCP 请求。
 * 不是合法 JSON 或结构不符 → AM_HUB_ERR_INVALID_JSON；登记不合法 → AM_HUB_ERR_INVALID_CONFIG（之前的登记继续生效）。 */
AmHubStatus am_hub_set_agents(AmHub *hub, const char *agents_json);

/* ---------------------------------------------------------------------------
 * 工具格式导出与分派（spec/hub-api.md 第 5 节）
 * ------------------------------------------------------------------------- */

/* 按格式导出工具定义（名称已编码为 [a-zA-Z0-9_-]{1,64}）。filter_json 同 am_hub_tools_json。 */
AmHubStatus am_hub_export_tools(const AmHub *hub, AmHubToolFormat format, const char *filter_json,
                                char **out_json);
/* 执行模型返回的一个工具调用。session 可为 NULL（默认会话）。cb 收到该格式的"工具结果"消息 JSON
 * （如 Anthropic 的 {"type":"tool_result","tool_use_id",…}）；执行失败也以该格式的错误结果返回。 */
AmHubStatus am_hub_dispatch(AmHub *hub, AmHubToolFormat format, const char *tool_call_json,
                            const char *session, AmHubResultFn cb, void *user_data);

/* ---------------------------------------------------------------------------
 * 事件与策略回调（cb 为 NULL 表示清除；替换 / 清除时释放旧的 user_data）
 * ------------------------------------------------------------------------- */

AmHubStatus am_hub_set_event_cb(AmHub *hub, AmHubEventFn cb, void *user_data, AmHubFreeFn free_user_data);

/* 审批回调（配合 config 的 approval.requireAtOrAbove）。未设置或清除后，需要审批的调用以 USER_REJECTED 结束。 */
AmHubStatus am_hub_set_approval_cb(AmHub *hub, AmHubApprovalFn cb, void *user_data, AmHubFreeFn free_user_data);
/* 完成并消费审批句柄。approved = false → 调用以 USER_REJECTED 结束。 */
AmHubStatus am_hub_approval_complete(AmHubApproval *approval, bool approved);

/* 配对回调。设置后，无静态清单或 Origin 不在白名单的 App 首次连接时询问（握手先返回 pending）。
 * 注意：设置过之后再清除（cb = NULL），未知 App 一律拒绝（不会恢复为未设置时的白名单行为）。 */
AmHubStatus am_hub_set_pairing_cb(AmHub *hub, AmHubPairingFn cb, void *user_data, AmHubFreeFn free_user_data);
AmHubStatus am_hub_pairing_complete(AmHubPairing *pairing, bool approved);

/* v2：自定义唤醒（替换默认的系统唤醒实现；cb = NULL 恢复配置 waker 决定的实现，默认系统唤醒）。调用休眠实例的工具、或 App 未运行
 * 而清单声明了 wake 时，Hub 生成令牌、发 {"type":"appWaking"} 事件并调用 cb（见 AmHubWakerFn）。
 * 同一目标的并发调用共用一次唤醒。 */
AmHubStatus am_hub_set_waker_cb(AmHub *hub, AmHubWakerFn cb, void *user_data, AmHubFreeFn free_user_data);
/* v2：完成并消费唤醒句柄。ok = true：已发出激活。ok = false：唤醒失败，调用以 error_kind（协议错误类别，
 * 如 "LAUNCH_FAILED"、"APP_NOT_INSTALLED"；NULL 或未知值按 "LAUNCH_FAILED"）与 message（可为 NULL）结束。
 * 句柄未完成就被丢弃（如 am_hub_free）按 LAUNCH_FAILED 处理。唤醒已超时或 Hub 已停止时返回
 * AM_HUB_ERR_ALREADY_COMPLETED（句柄仍被消费）。 */
AmHubStatus am_hub_waker_complete(AmHubWake *wake, bool ok, const char *error_kind, const char *message);

#ifdef __cplusplus
}
#endif

#endif /* APP_MCP_HUB_H */
