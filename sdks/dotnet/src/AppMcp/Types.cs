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
    public AppOverview? Overview { get; init; }

    /// <summary>生命周期策略（spec/lifecycle.md）。为 null 时 persistent（不休眠）。</summary>
    public LifecycleOptions? Lifecycle { get; init; }

    /// <summary>建立连接的超时；为 null 时 5 秒。</summary>
    public TimeSpan? ConnectTimeout { get; init; }

    /// <summary>心跳策略（spec/lifecycle.md 第 11 节 A3）。默认 <see cref="HeartbeatMode.Auto"/>。</summary>
    public HeartbeatMode Heartbeat { get; init; } = HeartbeatMode.Auto;

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
}
