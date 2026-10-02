// 运行状态（HubStatusInfo 及其组成，spec/hub-api.md 3.9；与 GET /status 相同）。
using System.Text.Json;
using System.Text.Json.Nodes;
using System.Text.Json.Serialization;

namespace AppMcp.Hub;

/// <summary>实例信息（InstanceInfo）。</summary>
public sealed record InstanceInfo(
    string InstanceId,
    string ClientKind,
    string Visibility,
    bool Focused,
    ulong LastActiveMs,
    string? Title)
{
    /// <summary>Hub 分配的连接 ID（"&lt;标记&gt;-&lt;序号&gt;"，与 Hub / SDK 日志的 cid 相同）；休眠实例为 null。</summary>
    public string? ConnectionId { get; init; }
}

/// <summary>运行状态（HubStatus，与 GET /status 相同，spec/hub-api.md 3.9）。</summary>
public sealed record HubStatusInfo(
    string Service,
    string Version,
    string? User,
    uint Pid,
    string? Listen,
    string? IpcEndpoint,
    ulong StartedAtMs,
    bool McpHttp,
    HubAuthStatus Auth,
    int McpSessions,
    IReadOnlyList<AppStatusInfo> Apps,
    IReadOnlyList<DiagnosticReport> Reports)
{
    /// <summary>租约策略与统计（spec/lifecycle.md 第 13 节 B2）；旧 Hub 为 null。</summary>
    public LeaseStatusInfo? Lease { get; init; }
    /// <summary>资源保护策略（全部字段给出）；旧 Hub 为 null。</summary>
    public HubLimits? Limits { get; init; }
    /// <summary>结果与 outputSchema 不符时的处理；旧 Hub 为 null。</summary>
    public OutputValidation? OutputValidation { get; init; }
    /// <summary>策略规则、命中次数与最近的加载错误；旧 Hub 为 null。</summary>
    public HubPolicyStatusInfo? Policy { get; init; }
    /// <summary>休眠记录持久化状态；未配置 <see cref="HubOptions.StateDir"/> 或旧 Hub 时为 null。</summary>
    public DormantStoreStatusInfo? DormantStore { get; init; }
    /// <summary>Agent 任务（调用方的跨请求状态，spec/hub-api.md 3.6），按 Caller 排序；旧 Hub 为 null。</summary>
    public IReadOnlyList<AgentTaskStatusInfo>? Tasks { get; init; }
    /// <summary>进行中的 subscriptions/listen 流数（spec/hub-api.md 3.6「通知」；McpSessions 只计 legacy 会话）；旧 Hub 为 null。</summary>
    public int? McpListenStreams { get; init; }
    /// <summary>已登记的 Agent 名（第 16 项 N5，不含令牌）；旧 Hub 为 null。</summary>
    public IReadOnlyList<string>? Agents { get; init; }
    /// <summary>按调用方记账（第 16 项 P3），按 Subject 排序；旧 Hub 为 null。</summary>
    public IReadOnlyList<UsageStatusInfo>? Usage { get; init; }
}

/// <summary>
/// 一个记账主体的用量（第 16 项 P3）。Subject：agent:&lt;名&gt; / local / api / other；Wakes：为该主体发起的唤醒
/// （与进行中的唤醒合并的也计入）；Apps：按 App 细分；AppsTruncated：细分条目达上限后新 App 只计入合计。
/// </summary>
public sealed record UsageStatusInfo(
    string Subject,
    ulong Calls,
    ulong Wakes,
    ulong RateLimited,
    ulong ArgumentsBytes,
    ulong ResultBytes,
    IReadOnlyList<AppUsageStatusInfo> Apps)
{
    public string? Agent { get; init; }
    public bool AppsTruncated { get; init; }
}

/// <summary>一个 App 上的用量（UsageStatusInfo.Apps 的一项）。</summary>
public sealed record AppUsageStatusInfo(string AppId, ulong Calls, ulong Wakes, ulong RateLimited, ulong ArgumentsBytes, ulong ResultBytes);

/// <summary>
/// 一个 Agent 任务（AgentTaskStatus）。Id：Hub 签发的 "task-&lt;128 位十六进制&gt;"；Caller：调用方键 "mcp:&lt;n&gt;" /
/// "principal:&lt;主体&gt;" / "api" / "api:&lt;session&gt;"；Kind：mcpSession / principal / api；Selections：未过期的 apps.select 选择；
/// Leases：本任务发出、尚未到期且实例仍连接的租约；Inflight：进行中的请求数。
/// </summary>
public sealed record AgentTaskStatusInfo(
    string Id,
    string Caller,
    string Kind,
    IReadOnlyList<TaskSelectionStatusInfo> Selections,
    IReadOnlyList<TaskLeaseStatusInfo> Leases,
    uint Inflight)
{
    /// <summary>距最近一次请求活动的毫秒数；没有活动记录时为 null。</summary>
    public ulong? IdleMs { get; init; }
    /// <summary>发起方 Agent 名（第 16 项 N5）；本机主体与 Hub API 为 null。</summary>
    public string? Agent { get; init; }
}

/// <summary>一项 apps.select 选择；ExpiresInMs：距失效的毫秒数（主体级选择），不单独过期时为 null。</summary>
public sealed record TaskSelectionStatusInfo(string AppId, string InstanceId)
{
    public ulong? ExpiresInMs { get; init; }
}

/// <summary>一项租约：实例的连接 ID 与距到期的毫秒数。</summary>
public sealed record TaskLeaseStatusInfo(string ConnectionId, ulong ExpiresInMs);

/// <summary>
/// 休眠记录持久化状态（DormantStoreStatus）。Dir：&lt;StateDir&gt;/dormant；LoadedInstances / ExpiredInstances：启动时读回 / 因过期丢弃的实例数；
/// Writes：启动以来成功写入 / 删除文件的次数；Issues：启动时跳过的文件（损坏、版本未知、超出上限）。
/// </summary>
public sealed record DormantStoreStatusInfo(
    string Dir,
    ulong LoadedInstances,
    ulong ExpiredInstances,
    ulong Writes,
    IReadOnlyList<StoreIssueInfo> Issues)
{
    /// <summary>最近一次写入失败。</summary>
    public string? LastError { get; init; }
}

/// <summary>被跳过的文件（File：相对于休眠记录目录）与中文说明。</summary>
public sealed record StoreIssueInfo(string File, string Reason);

/// <summary>策略状态（PolicyStatus）：生效的规则（按顺序）与命中次数、规则集生效时刻（Unix 毫秒）。</summary>
public sealed record HubPolicyStatusInfo(IReadOnlyList<HubPolicyRuleStatus> Rules, ulong LoadedAtMs)
{
    /// <summary>最近一次 <see cref="AppMcpHub.SetPolicy"/> 失败的原因（之前的规则继续生效）；之后成功加载时清除。</summary>
    public HubPolicyLoadError? LastError { get; init; }
}

/// <summary>一条生效的规则；Hits 为自本规则集生效以来拒绝或按不存在处理的调用 / 唤醒次数（列表过滤不计）。</summary>
public sealed record HubPolicyRuleStatus(string Id, HubPolicyAction Action, string App, ulong Hits)
{
    public string? Tool { get; init; }
    public HubAnnotationMatch? Annotations { get; init; }
    public string? Agent { get; init; }
    public IReadOnlyList<HubPolicyHook>? Hooks { get; init; }
}

/// <summary>规则加载失败（AtMs：Unix 毫秒）。</summary>
public sealed record HubPolicyLoadError(string Message, ulong AtMs);

/// <summary>租约策略与统计（LeaseStatus）。Mode：adaptive / fixed（固定 LeaseTtl）/ off（LeaseTtl = 0）。</summary>
public sealed record LeaseStatusInfo(
    string Mode,
    ulong DefaultMs,
    ulong MinMs,
    ulong MaxMs,
    ulong MarginMs,
    uint Window,
    ulong IdleRevokeMs,
    ulong AdaptiveGrants,
    ulong DefaultGrants,
    ulong RevokedSessionEnd,
    ulong RevokedIdle,
    IReadOnlyList<LeasePairStatusInfo> Pairs);

/// <summary>一个（会话, App）的租约统计。Session：MCP "mcp:&lt;n&gt;"，API "api" / "api:&lt;session&gt;"。</summary>
public sealed record LeasePairStatusInfo(string Session, string AppId, uint Samples, ulong NextTtlMs, bool Adaptive);

/// <summary>
/// 每实例功耗观测（InstancePower，spec/lifecycle.md 第 12 节；跨重连与休眠保留，Hub 重启清零）。
/// HeartbeatMs：SDK 声明的心跳间隔，0 = 不发（本地传输），null = 旧 SDK；LifecycleMode：persistent / idle / on-demand，null = 未声明；
/// AwakeReasons：见 <see cref="HubAwakeReasons"/>，休眠实例与可以休眠时为空。
/// </summary>
public sealed record InstancePowerInfo(
    ulong Reconnects,
    ulong Wakes,
    ulong OnlineSecs,
    ulong Heartbeats,
    ulong? HeartbeatMs,
    string? LifecycleMode,
    IReadOnlyList<string>? AwakeReasons);

/// <summary>已连接实例当前不能休眠的原因（Hub 可见部分；App 的 hold() 只有 SDK 知道）。</summary>
public static class HubAwakeReasons
{
    public const string Persistent = "persistent";
    public const string Call = "call";
    public const string Lease = "lease";
    /// <summary>Host 订阅了该实例声明 realtime 的资源（spec/lifecycle.md 第 13 节 B3）。</summary>
    public const string Subscription = "subscription";
    public const string WakePending = "wake-pending";
}

/// <summary>主 HTTP 服务的令牌策略（AuthStatus）。</summary>
public sealed record HubAuthStatus(bool TokenConfigured, bool TokenRequiredWithoutOrigin);

/// <summary>App 状态（AppStatus）。State：connected / waking / dormant / disconnected。</summary>
public sealed record AppStatusInfo(
    string AppId,
    string Name,
    string Kind,
    string State,
    IReadOnlyList<InstanceStatusInfo> Instances,
    LastErrorInfo? LastError)
{
    /// <summary>Hub 启动以来为该 App 实际发出的唤醒激活次数（含冷启动；上游为 0）。</summary>
    public ulong Wakes { get; init; }
    /// <summary>Hub 启动以来该 App 的调用被限流（RATE_LIMITED）的次数。</summary>
    public ulong RateLimited { get; init; }
    /// <summary>Hub 启动以来该 App 的参数 / 结果 / 资源超过大小上限（PAYLOAD_TOO_LARGE）的次数。</summary>
    public ulong TooLarge { get; init; }
    /// <summary>该 App 的工具声明（核对注解与 outputSchema）；旧 Hub 为空。</summary>
    public IReadOnlyList<ToolDeclarationInfo> Tools { get; init; } = [];
}

/// <summary>一个工具的声明（ToolDeclaration）。Name 为局部名；Annotations 为 App 声明的原样注解（未声明为 null）；
/// Effective 为 Agent 实际看到的注解（声明优先、缺少的按 Risk 推导）；OutputSchema 表示是否声明了 outputSchema。</summary>
public sealed record ToolDeclarationInfo(
    string Name,
    string Risk,
    HubToolAnnotations? Annotations,
    HubToolAnnotations Effective,
    bool OutputSchema);

/// <summary>实例状态（InstanceStatus = InstanceInfo + state）。State：connected / dormant / waking。</summary>
public sealed record InstanceStatusInfo(
    string InstanceId,
    string ClientKind,
    string Visibility,
    bool Focused,
    ulong LastActiveMs,
    string? Title,
    uint? Pid,
    string? ConnectionId,
    string State)
{
    /// <summary>功耗观测；Hub 尚无该实例的计数时为 null。</summary>
    public InstancePowerInfo? Power { get; init; }
}

/// <summary>最近一次错误（LastError）。Code 为连接级错误码或工具错误类别，未知时为 null；上游错误的 AtMs 为 0。</summary>
public sealed record LastErrorInfo(string? Code, string Message, ulong AtMs);

/// <summary>一条 SDK 诊断上报（DiagnosticReport，app/diagnostic）。</summary>
public sealed record DiagnosticReport(
    string AppId,
    string InstanceId,
    string ConnectionId,
    string Code,
    string Message,
    uint Count,
    ulong ReceivedAtMs);
