using System.Text.Json;
using System.Text.Json.Serialization;

namespace AppMcp;

/// <summary>与 C 接口 AmStatus 对应的状态码。</summary>
public enum AppMcpStatus
{
    Ok = 0,
    InvalidArgument = 1,
    InvalidName = 2,
    InvalidSchema = 3,
    DuplicateName = 4,
    InvalidJson = 5,
    InvalidConfig = 6,
    AlreadyCompleted = 7,
    Disposed = 8,
    Stopped = 9,
    Internal = 10,
    Panic = 11,
}

public enum ClientKind { Native = 0, Hybrid = 1 }

/// <summary>风险等级，决定 Host 的确认策略。</summary>
public enum ToolRisk { Read = 0, Write = 1, Destructive = 2, Payment = 3, OsSensitive = 4 }

/// <summary>调用时 App 需要的激活方式。</summary>
public enum ToolActivation { Headless = 0, Background = 1, Foreground = 2 }

public enum AppVisibility { Visible = 0, Hidden = 1, Frozen = 2 }

/// <summary>用户正在操作（<see cref="AppMcpClient.SetBusy"/>）期间写调用的处理方式（spec/protocol.md 5.3）。</summary>
public enum BusyPolicy
{
    /// <summary>以 RATE_LIMITED（details <c>{"scope":"busy"}</c>）拒绝（默认）。</summary>
    Reject = 0,
    /// <summary>排队，用户操作结束后按到达顺序执行（仍受 <see cref="AppMcpClientOptions.MaxQueuedCalls"/> 与调用超时约束）。</summary>
    Queue = 1,
}

public enum CancelReason { Requested = 0, Timeout = 1, Disconnected = 2, Stopped = 3 }

public enum ClientStatus
{
    Idle = 0,
    Connecting = 1,
    Handshaking = 2,
    PendingPairing = 3,
    Connected = 4,
    Backoff = 5,
    Rejected = 6,
    Stopped = 7,
    /// <summary>已休眠：无连接、无定时器，等待唤醒（spec/lifecycle.md）。</summary>
    Dormant = 8,
    /// <summary>收到唤醒后正在回连。</summary>
    Waking = 9,
    /// <summary>对端不是期望的 Host（不是 app-mcp，或属于其他用户；spec/protocol.md 1.6）。不再自动重连，
    /// Wake / ConnectNow 时再试一次；原因见 <see cref="ClientState.Reason"/>。</summary>
    HostMismatch = 10,
}

public enum LogLevel { Debug = 0, Info = 1, Warn = 2, Error = 3 }

/// <summary>协议错误类别（spec/protocol.md 第 4 节）。</summary>
public enum ToolErrorKind
{
    ToolNotFound,
    ToolDisabled,
    InvalidInput,
    UserRejected,
    Timeout,
    HandlerError,
    Cancelled,
    AppDisconnected,
    AppNotInstalled,
    LaunchFailed,
    AppNotResponding,
    InstanceFrozen,
    ResourceNotFound,
    Unauthorized,
    UnsupportedProtocol,
    /// <summary>Host 侧限流（由 Host 产生，App 一般不用）。</summary>
    RateLimited,
    /// <summary>调用参数、结果或资源内容超过 Host 的大小上限（由 Host 产生，App 一般不用）。</summary>
    PayloadTooLarge,
    /// <summary>调用被用户 / 厂商的策略规则拒绝（由 Host 产生，App 一般不用）。</summary>
    PolicyDenied,
    /// <summary>需要用户本人操作后才能继续（登录过期、系统权限未授予、需切到前台、需在 App 内确认等）；
    /// 用 <see cref="UserActionRequiredException"/> 抛出可附带 reason / uri。</summary>
    UserActionRequired,
    /// <summary>导航没有完成（不支持 / 出错 / 超时 / 导航后工具未出现，spec/protocol.md 3.4）。</summary>
    NavigationFailed,
    /// <summary>导航被拒绝（App 拒绝或页面不可由 Agent 导航）。</summary>
    NavigationDenied,
    /// <summary>对象锁冲突（由 Host 产生，App 一般不用；spec/hub-api.md 3.6「对象锁」）。</summary>
    Locked,
}

public static class ToolErrorKinds
{
    /// <summary>协议中的字符串形式，如 <c>USER_REJECTED</c>。</summary>
    public static string ToProtocolString(this ToolErrorKind kind) => kind switch
    {
        ToolErrorKind.ToolNotFound => "TOOL_NOT_FOUND",
        ToolErrorKind.ToolDisabled => "TOOL_DISABLED",
        ToolErrorKind.InvalidInput => "INVALID_INPUT",
        ToolErrorKind.UserRejected => "USER_REJECTED",
        ToolErrorKind.Timeout => "TIMEOUT",
        ToolErrorKind.HandlerError => "HANDLER_ERROR",
        ToolErrorKind.Cancelled => "CANCELLED",
        ToolErrorKind.AppDisconnected => "APP_DISCONNECTED",
        ToolErrorKind.AppNotInstalled => "APP_NOT_INSTALLED",
        ToolErrorKind.LaunchFailed => "LAUNCH_FAILED",
        ToolErrorKind.AppNotResponding => "APP_NOT_RESPONDING",
        ToolErrorKind.InstanceFrozen => "INSTANCE_FROZEN",
        ToolErrorKind.ResourceNotFound => "RESOURCE_NOT_FOUND",
        ToolErrorKind.Unauthorized => "UNAUTHORIZED",
        ToolErrorKind.UnsupportedProtocol => "UNSUPPORTED_PROTOCOL",
        ToolErrorKind.RateLimited => "RATE_LIMITED",
        ToolErrorKind.PayloadTooLarge => "PAYLOAD_TOO_LARGE",
        ToolErrorKind.PolicyDenied => "POLICY_DENIED",
        ToolErrorKind.UserActionRequired => "USER_ACTION_REQUIRED",
        ToolErrorKind.NavigationFailed => "NAVIGATION_FAILED",
        ToolErrorKind.NavigationDenied => "NAVIGATION_DENIED",
        ToolErrorKind.Locked => "LOCKED",
        _ => "HANDLER_ERROR",
    };
}

/// <summary>库函数失败时抛出。</summary>
public class AppMcpException : Exception
{
    public AppMcpException(AppMcpStatus status, string message) : base(message) => Status = status;
    public AppMcpStatus Status { get; }
}

/// <summary>在 handler 中抛出，以指定错误类别失败（其他异常按 HANDLER_ERROR 处理）。</summary>
public class ToolCallException : Exception
{
    public ToolCallException(ToolErrorKind kind, string message) : base(message) => Kind = kind;
    public ToolCallException(ToolErrorKind kind, string message, Exception inner) : base(message, inner) => Kind = kind;

    /// <summary>附带结构化详情：用客户端的序列化选项转成 JSON；对象的字段合并进错误的 <c>data</c>，其他值放在 <c>data.details</c>。</summary>
    public ToolCallException(ToolErrorKind kind, string message, object? details) : base(message)
    {
        Kind = kind;
        Details = details;
    }

    public ToolErrorKind Kind { get; }

    /// <summary>结构化详情（可为 null）。</summary>
    public object? Details { get; }
}

/// <summary><c>USER_ACTION_REQUIRED</c> 的 <c>data.reason</c> 建议取值（spec/protocol.md 第 4 节；也可用其他字符串）。</summary>
public static class UserActionReason
{
    /// <summary>登录已过期 / 未登录。</summary>
    public const string Login = "login";
    /// <summary>系统权限未授予（相机、位置、通知等）。</summary>
    public const string Permission = "permission";
    /// <summary>需要把 App 切到前台。</summary>
    public const string Foreground = "foreground";
    /// <summary>需要用户在 App 内确认。</summary>
    public const string Confirm = "confirm";
}

/// <summary>在 handler 中抛出，以 <c>USER_ACTION_REQUIRED</c> 失败（app_mcp.h v11）：需要用户本人操作后才能继续。
/// 导航回调中抛出同样以 USER_ACTION_REQUIRED 结束导航（v15，如后台时发通知后以 reason "foreground" 与 uri 回复）。</summary>
/// <param name="message">面向用户的说明（Agent 转告用户）。</param>
/// <param name="reason">可选类别，见 <see cref="UserActionReason"/>；为 null 时不出现在错误的 data 中。</param>
/// <param name="uri">可选的 App 内入口（深链接等）；为 null 时不出现在错误的 data 中。</param>
public class UserActionRequiredException(string message, string? reason = null, string? uri = null)
    : ToolCallException(ToolErrorKind.UserActionRequired, message)
{
    /// <summary>类别（<c>data.reason</c>），可为 null。</summary>
    public string? Reason { get; } = reason;

    /// <summary>App 内入口（<c>data.uri</c>），可为 null。</summary>
    public string? Uri { get; } = uri;
}

/// <summary>客户端状态。</summary>
/// <param name="Status">状态。</param>
/// <param name="RetryIn"><see cref="ClientStatus.Backoff"/> 时距下一次重连的时间。</param>
/// <param name="Reason"><see cref="ClientStatus.Rejected"/> / <see cref="ClientStatus.HostMismatch"/> 时的原因；
/// <see cref="ClientStatus.Backoff"/> 时为连接失败 / 断开的原因（有的话）。</param>
public readonly record struct ClientState(ClientStatus Status, TimeSpan? RetryIn, string? Reason)
{
    /// <summary>与 <see cref="Reason"/> 对应的错误码（spec/protocol.md 10.1，如 <c>HOST_NOT_RUNNING</c>）；
    /// Rejected / HostMismatch 时总有，Backoff 时在连接失败等情况下有，其他状态为 null。</summary>
    /// <remarks>@compat 用 init 属性而非新增位置参数，保持构造函数与解构的源码兼容。</remarks>
    public string? Code { get; init; }

    /// <summary>只有这些状态带错误码（spec/protocol.md 10.1）。</summary>
    internal static bool StatusHasCode(ClientStatus status) =>
        status is ClientStatus.Backoff or ClientStatus.Rejected or ClientStatus.HostMismatch;
}

public sealed class ClientStateChangedEventArgs(ClientState state) : EventArgs
{
    public ClientState State { get; } = state;
}

public sealed class PairedEventArgs(string token) : EventArgs
{
    /// <summary>新 token，App 应持久化，下次放入 <see cref="AppMcpClientOptions.Token"/>。</summary>
    public string Token { get; } = token;
}

public sealed class LogEventArgs(LogLevel level, string message) : EventArgs
{
    public LogLevel Level { get; } = level;
    public string Message { get; } = message;
}

/// <summary>调用去重策略（<see cref="AppMcpClientOptions.CallDedup"/>）。任一项为 0 关闭去重。</summary>
public sealed record CallDedupOptions
{
    /// <summary>首次结果的保留时长。默认 5 分钟。</summary>
    public TimeSpan Ttl { get; init; } = TimeSpan.FromMinutes(5);
    /// <summary>最多保留的结果数（超出淘汰最早的）。默认 64。</summary>
    public int MaxEntries { get; init; } = 64;

    /// <summary>关闭去重。</summary>
    public static CallDedupOptions Off { get; } = new() { Ttl = TimeSpan.Zero, MaxEntries = 0 };
}

/// <summary>App 总览：Host 在模型首次接触该 App 时附带。</summary>
public sealed record AppOverview(string Summary, string? Body = null, string? Locale = null);

public sealed class AppMcpClientOptions
{
    private readonly SynchronizationContext? _dispatcher;
    private readonly bool _dispatcherSet;

    /// <summary>必填，<c>[a-z][a-z0-9-]{0,62}</c>。</summary>
    public required string AppId { get; init; }
    public required string AppName { get; init; }
    /// <summary>为 null 时自动生成。</summary>
    public string? InstanceId { get; init; }
    /// <summary>
    /// Host 端点："unix:&lt;绝对路径&gt;"、"pipe:\\.\pipe\&lt;名称&gt;"、"ws://…" 或 "wss://…"（spec/protocol.md 第 1 节）。
    /// 为 null 时：环境变量 APP_MCP_ENDPOINT → 登记文件 ~/.app-mcp/run/endpoints.json → 平台默认本地 IPC 端点（Windows 为 \\.\pipe\app-mcp-&lt;用户 SID&gt;）→ ws://127.0.0.1:7717/app。
    /// </summary>
    public string? HostUrl { get; init; }
    public string? AppVersion { get; init; }
    public string? InstanceTitle { get; init; }
    /// <summary>之前配对得到的 token（见 <see cref="AppMcpClient.Paired"/>）。</summary>
    public string? Token { get; init; }
    /// <summary>为 null 时读取环境变量 APP_MCP_LAUNCH_TOKEN。</summary>
    public string? LaunchToken { get; init; }
    public ClientKind ClientKind { get; init; } = ClientKind.Native;
    /// <summary>同时执行的调用上限。</summary>
    public int MaxConcurrentCalls { get; init; } = 1;
    /// <summary>排队中（等并发名额 / 互斥组）的调用上限；超出时新调用以 RATE_LIMITED（details scope "queue"）拒绝。
    /// 0 = 不限；不能为负数（spec/protocol.md 5.3）。</summary>
    public int MaxQueuedCalls { get; init; } = 64;
    /// <summary>用户正在操作（<see cref="AppMcpClient.SetBusy"/>）期间写调用的处理方式；默认 <see cref="AppMcp.BusyPolicy.Reject"/>。
    /// 运行中可用 <see cref="AppMcpClient.SetBusyPolicy"/> 修改（spec/protocol.md 5.3）。</summary>
    public BusyPolicy BusyPolicy { get; init; } = BusyPolicy.Reject;
    public AppOverview? Overview { get; init; }

    /// <summary>生命周期策略（spec/lifecycle.md）。为 null 时 persistent（不休眠）。</summary>
    public LifecycleOptions? Lifecycle { get; init; }

    /// <summary>建立连接的超时；为 null 时 5 秒。</summary>
    public TimeSpan? ConnectTimeout { get; init; }

    /// <summary>心跳策略（spec/lifecycle.md 第 11 节 A3）。默认 <see cref="HeartbeatMode.Auto"/>。</summary>
    public HeartbeatMode Heartbeat { get; init; } = HeartbeatMode.Auto;

    /// <summary>调用去重（spec/protocol.md 3.3）：同一 callId 在有效期内重复到达时重放首次结果、不再执行 handler。
    /// 为 null 时默认（保留 5 分钟、最多 64 条）；见 <see cref="CallDedupOptions"/>。</summary>
    public CallDedupOptions? CallDedup { get; init; }

    /// <summary>按名寻址（spec/naming.md）：<see cref="AppMcpClient.Start"/> 后在系统名字服务登记，由 Hub 按名拨入
    /// （Linux：D-Bus 会话总线名 <c>dev.appmcp.App.&lt;AppId&gt;</c>；Windows：命名管道 <c>\\.\pipe\appmcp-&lt;用户 SID&gt;-&lt;AppId&gt;</c>）。
    /// 需先用 <c>app-mcp-host app install --app-id &lt;AppId&gt; --exec &lt;本程序&gt;</c> 登记，Hub 以 <c>--name-service</c> 运行。
    /// 通常与 <see cref="LifecycleMode.OnDemand"/> + <see cref="Residency.ExitWhenIdle"/> 同用：由激活启动（命令行带
    /// <c>--app-mcp-activation</c>）的进程在通道关闭后收到 <see cref="AppMcpClient.IdleExit"/>。本平台不支持时经
    /// <see cref="AppMcpClient.Log"/> 报告，其余照常。默认 false。</summary>
    public bool RegisterName { get; init; }

    /// <summary>登记实例名（<c>[a-z][a-z0-9-]{0,31}</c>，不能是 <c>default</c>）：在默认名字之外另登记
    /// <c>dev.appmcp.App.&lt;AppId&gt;.&lt;实例&gt;</c>（Windows 管道 <c>…-&lt;AppId&gt;.&lt;实例&gt;</c>），只在本进程运行期间存在。
    /// 只在 <see cref="RegisterName"/> 为 true 时有意义；不合法时 <see cref="AppMcpClient.Create"/> 抛出
    /// <see cref="AppMcpException"/>（<see cref="AppMcpStatus.InvalidConfig"/>）。为 null 时不登记实例名。</summary>
    public string? NameInstance { get; init; }

    /// <summary>App 在后台（<see cref="AppVisibility"/> 非 Visible）时是否仍把导航交给导航回调（spec/protocol.md 3.4「后台与前台」）。
    /// 为 null 时用平台默认（Windows / Linux / macOS 桌面为 true：回调可自行激活窗口）；false 时直接以 USER_ACTION_REQUIRED
    /// （reason "foreground"）回复。运行中可用 <see cref="AppMcpClient.SetNavigateInBackground"/> 修改。</summary>
    public bool? NavigateInBackground { get; init; }

    /// <summary>
    /// handler 与事件执行的线程。未设置时捕获 <see cref="AppMcpClient.Create"/> 调用时的
    /// <see cref="SynchronizationContext.Current"/>（在 WPF / WinUI 的 UI 线程创建即自动回到 UI 线程）；
    /// 显式设为 null 时 handler 在线程池执行、事件在库的分发线程上触发。
    /// </summary>
    public SynchronizationContext? Dispatcher
    {
        get => _dispatcher;
        init
        {
            _dispatcher = value;
            _dispatcherSet = true;
        }
    }

    internal bool DispatcherSet => _dispatcherSet;

    /// <summary>参数反序列化、结果序列化、Schema 生成使用的选项。默认 <see cref="JsonSerializerOptions.Web"/>。</summary>
    public JsonSerializerOptions? SerializerOptions { get; init; }
}

public sealed class ToolOptions
{
    /// <summary>JSON Schema 文本（type 必须为 object）。类型化注册时为 null 则由参数类型生成。</summary>
    public string? InputSchemaJson { get; init; }
    /// <summary>旧写法：优先用 <see cref="Annotations"/>。两者同时声明时注解中的字段优先，缺少的按 Risk 推导。</summary>
    public ToolRisk Risk { get; init; } = ToolRisk.Write;
    /// <summary>标准 MCP 工具注解，原样转发给 Agent；为 null 时不声明（Host 按 <see cref="Risk"/> 推导）。</summary>
    public ToolAnnotations? Annotations { get; init; }
    /// <summary>结果的 JSON Schema 文本（MCP outputSchema）；为 null 时不声明。可用 <see cref="ToolSchema.For{T}"/> 由结果类型生成。</summary>
    public string? OutputSchemaJson { get; init; }
    /// <summary>为 null 时使用 Host 默认值。</summary>
    public ToolActivation? Activation { get; init; }
    public string? Title { get; init; }
    public bool Enabled { get; init; } = true;
    /// <summary>对界面的依赖（spec/protocol.md 3.4）：<see cref="ToolSurface.View"/> 的工具只在所在界面可见且处于最上层时注册
    /// （WPF / WinUI 可用 AppMcp.Wpf / AppMcp.WinUI 的可见性绑定）。</summary>
    public ToolSurface Surface { get; init; } = ToolSurface.App;
    /// <summary>所在页面名（<c>[a-zA-Z0-9_.-]{1,64}</c>）；为 null 时不声明。Hub 在该工具未注册时据此导航
    /// （<see cref="AppMcpClient.SetNavigationHandler(Func{NavigationRequest, Task}?)"/>）。</summary>
    public string? Page { get; init; }
    /// <summary>只对 <see cref="ToolSurface.View"/> 有意义：App 在后台、本工具不可调用时 Hub 改调的同 App app 工具本地名；
    /// 为 null 时不声明（spec/protocol.md 3.4「后台与前台」）。</summary>
    public string? BackgroundTool { get; init; }
    /// <summary>本工具同时执行的调用上限；0 = 不单独限制，只受 <see cref="AppMcpClientOptions.MaxConcurrentCalls"/> 约束；不能为负数。
    /// 只在 SDK 内调度，不同步给 Host（spec/protocol.md 5.3）。</summary>
    public int Concurrency { get; init; }
    /// <summary>互斥组名（<c>[a-zA-Z0-9_.-]{1,64}</c>）：同组的工具同一时刻至多一个在执行；为 null 时不互斥（spec/protocol.md 5.3）。</summary>
    public string? Exclusive { get; init; }
    /// <summary>实现的标准意图（spec/intents.md），每项 <c>"&lt;动词&gt;@&lt;主版本&gt;"</c>（如 <c>"message.send@1"</c>），最多 4 项、不重复；
    /// 为 null 或空时不声明。格式不合法时注册 / 更新抛出 <see cref="AppMcpStatus.InvalidName"/> 的 <see cref="AppMcpException"/>。
    /// Agent 用内置工具 <c>apps.intents</c> 按动词找到实现者。</summary>
    public IReadOnlyList<string>? Implements { get; init; }
    /// <summary>结果缓存声明（spec/protocol.md 3.6）：Hub 在 TTL 内对相同参数的调用复用结果（命中不唤醒 App）。只对生效注解只读
    /// （<c>ReadOnlyHint</c> 为 true，或未声明注解时 <see cref="ToolRisk.Read"/>）的工具生效，否则照常注册并记警告日志。
    /// 为 null 时不声明（更新时清除）。</summary>
    public CachePolicy? Cache { get; init; }
    /// <summary>弃用声明（spec/protocol.md 3.7）：照常列出与调用，Hub 把声明原样交给 Agent 并在描述前标注。为 null 时不声明（更新时清除）。</summary>
    public ToolDeprecation? Deprecated { get; init; }
    /// <summary>本工具的成功结果可能带撤销信息（<see cref="ToolResult.Undo"/>，spec/protocol.md 3.8）：只用于展示（Agent 可提示
    /// "此操作可撤销"），不约束结果。false 时不声明（更新时清除）。</summary>
    public bool Undoable { get; init; }
}

/// <summary>工具弃用声明（spec/protocol.md 3.7）。<paramref name="Message"/>：1..=500 个字符，面向模型说明为什么弃用、该怎么做；
/// <paramref name="Replacement"/>：同一 App 中替代工具的局部名（不得指向自身）；<paramref name="Until"/>：计划移除日期（YYYY-MM-DD），只作提示。
/// 格式不合法时注册 / 更新抛出 <see cref="AppMcpStatus.InvalidConfig"/> 的 <see cref="AppMcpException"/>。</summary>
public sealed record ToolDeprecation(string Message, string? Replacement = null, string? Until = null);

/// <summary>结果缓存的范围（spec/protocol.md 3.6）。</summary>
public enum CacheScope
{
    /// <summary>按调用方隔离（缺省）。</summary>
    Private = 0,
    /// <summary>全体调用方共用：只用于与调用方无关的数据。</summary>
    Shared = 1,
}

/// <summary>工具 / 资源的结果缓存声明（spec/protocol.md 3.6）。<paramref name="TtlMs"/> 须在 1..=86400000 之间，否则注册 / 更新抛出
/// <see cref="AppMcpStatus.InvalidConfig"/> 的 <see cref="AppMcpException"/>。缓存多久、能否跨调用方共用由 App 判断，Hub 只执行。</summary>
public sealed record CachePolicy(ulong TtlMs, CacheScope Scope = CacheScope.Private);

/// <summary>工具对界面的依赖（spec/protocol.md 3.4）。</summary>
public enum ToolSurface
{
    /// <summary>不依赖界面：后台可调、可唤醒（缺省）。</summary>
    App = 0,
    /// <summary>依赖界面：只在所在界面可见且处于最上层时注册。</summary>
    View = 1,
}

/// <summary>标准 MCP 工具注解（spec/protocol.md 第 3 节）。本库不据此做判断，只原样转发；为 null 的字段不声明。</summary>
public sealed record ToolAnnotations
{
    /// <summary>给人看的工具标题。</summary>
    [JsonPropertyName("title")] public string? Title { get; init; }
    /// <summary>不修改任何状态。</summary>
    [JsonPropertyName("readOnlyHint")] public bool? ReadOnlyHint { get; init; }
    /// <summary>可能做出破坏性 / 不可撤销的修改（只在非只读时有意义）。</summary>
    [JsonPropertyName("destructiveHint")] public bool? DestructiveHint { get; init; }
    /// <summary>以相同参数重复调用没有额外效果（只在非只读时有意义）。</summary>
    [JsonPropertyName("idempotentHint")] public bool? IdempotentHint { get; init; }
    /// <summary>会与外部世界交互（网络、第三方、其他用户可见）。</summary>
    [JsonPropertyName("openWorldHint")] public bool? OpenWorldHint { get; init; }
}

/// <summary>内容面向谁（MCP 内容注解 audience）。</summary>
public enum ContentAudience { User, Assistant }

/// <summary>结果内容的标注（MCP 内容注解），Host 原样转发；为 null 的字段不声明。</summary>
public sealed record ContentAnnotations
{
    [JsonPropertyName("audience")] public IReadOnlyList<ContentAudience>? Audience { get; init; }
    /// <summary>重要程度，0（可选）到 1（必需）。</summary>
    [JsonPropertyName("priority")] public double? Priority { get; init; }
    /// <summary>最后修改时刻（ISO 8601）。</summary>
    [JsonPropertyName("lastModified")] public string? LastModified { get; init; }
}

/// <summary>调用结果的业务状态（spec/protocol.md 3.2）。数值与 C 接口 AmResultStatus 一致。</summary>
public enum ToolResultStatus
{
    /// <summary>已完成（缺省）。</summary>
    Done = 0,
    /// <summary>已受理、尚未完成（等待用户在 App 内确认或异步处理）；后续状态见 <see cref="ToolResult.StateResource"/>。</summary>
    Pending = 1,
    /// <summary>只完成了一部分，说明见 <see cref="ToolResult.Summary"/>。</summary>
    Partial = 2,
    /// <summary>没有做任何改动（目标状态已满足或无事可做）。</summary>
    Noop = 3,
}

/// <summary>
/// 结构化调用结果：handler 返回它（而不是普通值）时，除返回值外还带业务状态、摘要与内容注解。
/// 直接返回普通值 = 只有 <see cref="Data"/> 的 done 结果。
/// </summary>
public sealed record ToolResult
{
    public ToolResult(object? data = null) => Data = data;

    /// <summary>返回值，用客户端的序列化选项转成 JSON；null 表示无返回值（Host 对模型输出"已完成"）。</summary>
    public object? Data { get; init; }
    public ToolResultStatus Status { get; init; } = ToolResultStatus.Done;
    /// <summary><see cref="ToolResultStatus.Pending"/> 时可读取后续状态的资源名。</summary>
    public string? StateResource { get; init; }
    /// <summary>一句面向模型 / 用户的结论（Partial 时说明完成了哪部分）。</summary>
    public string? Summary { get; init; }
    public ContentAnnotations? Annotations { get; init; }
    /// <summary>调用后内容可能已变化的资源名；与 <see cref="ToolContext.AddStateHint"/> 添加的合并。</summary>
    public IReadOnlyList<string>? StateHints { get; init; }
    /// <summary>撤销本次调用的逆操作（spec/protocol.md 3.8）：Hub 记录后供 Agent 用 <c>apps.undo</c> 撤销；只在
    /// <see cref="ToolResultStatus.Done"/> / <see cref="ToolResultStatus.Partial"/> 时有效。为 null 时不可撤销。</summary>
    public UndoAction? Undo { get; init; }
}

/// <summary>撤销本次调用的逆操作（spec/protocol.md 3.8）：调用同一 App 的工具 <paramref name="Tool"/>（局部名，可为本工具自身），
/// 参数 <paramref name="Arguments"/>（用客户端的序列化选项转成 JSON 对象；null 表示 <c>{}</c>），<paramref name="Label"/> 为一句
/// 面向用户的说明（1..=200 个字符）。格式不合法（工具名、参数不是对象或超过 64 KiB、说明为空或超长）时 SDK 去掉撤销信息并记警告日志，
/// 结果其余部分照常发送。</summary>
public sealed record UndoAction(string Tool, object? Arguments = null, string? Label = null);
