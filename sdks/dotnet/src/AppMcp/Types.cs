using System.Text.Json;

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
public readonly record struct ClientState(ClientStatus Status, TimeSpan? RetryIn, string? Reason);

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
    /// 为 null 时：环境变量 APP_MCP_ENDPOINT → 平台默认本地 IPC 端点（Windows 为 \\.\pipe\app-mcp-&lt;用户 SID&gt;）→ ws://127.0.0.1:7717。
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
    public ToolRisk Risk { get; init; } = ToolRisk.Write;
    /// <summary>为 null 时使用 Host 默认值。</summary>
    public ToolActivation? Activation { get; init; }
    public string? Title { get; init; }
    public bool Enabled { get; init; } = true;
}
