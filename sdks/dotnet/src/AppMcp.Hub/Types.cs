using System.Text.Json;
using System.Text.Json.Nodes;

namespace AppMcp.Hub;

/// <summary>与 app_mcp_hub.h 的 AmHubStatus 一致。</summary>
public enum HubStatus
{
    Ok = 0,
    InvalidArgument = 1,
    InvalidJson = 2,
    InvalidConfig = 3,
    Io = 4,
    /// <summary>Hub 拒绝了操作；错误说明为 "&lt;KIND&gt;: &lt;message&gt;"。</summary>
    Hub = 5,
    /// <summary>审批 / 配对已超时或被取消。</summary>
    AlreadyCompleted = 6,
    Stopped = 7,
    Internal = 8,
    Panic = 9,
}

/// <summary>工具导出格式（spec/hub-api.md 第 5 节），与 AmHubToolFormat 一致。</summary>
public enum ToolFormat
{
    Mcp = 0,
    OpenAiChat = 1,
    OpenAiResponses = 2,
    Anthropic = 3,
    Gemini = 4,
}

/// <summary>风险等级（协议中的 kebab-case 字符串）。</summary>
public enum HubRisk { Read, Write, Destructive, Payment, OsSensitive }

public static class HubRiskExtensions
{
    public static string ToProtocolString(this HubRisk risk) => risk switch
    {
        HubRisk.Read => "read",
        HubRisk.Write => "write",
        HubRisk.Destructive => "destructive",
        HubRisk.Payment => "payment",
        HubRisk.OsSensitive => "os-sensitive",
        _ => throw new ArgumentOutOfRangeException(nameof(risk)),
    };
}

/// <summary>工具暴露方式（spec/hub-api.md 3.7）。</summary>
public enum ToolExposure
{
    /// <summary>App 与上游工具总数超过阈值时渐进暴露，否则全部列出（默认）。</summary>
    Auto,
    /// <summary>工具列表只含 apps.* 与本会话展开过（apps.tools）、调用过或选定了实例的 App 的工具。</summary>
    Progressive,
    /// <summary>全部列出。</summary>
    All,
}

/// <summary>唤醒器配置（spec/hub-api.md 3.5）；<see cref="AppMcpHub"/> 的自定义唤醒回调优先。</summary>
public sealed class WakerOptions
{
    private readonly JsonNode _json;
    private WakerOptions(JsonNode json) => _json = json;

    /// <summary>按平台执行系统激活（默认）。</summary>
    public static WakerOptions System { get; } = new(JsonValue.Create("system"));
    /// <summary>不唤醒：休眠实例 / 未运行 App 的调用直接返回 APP_DISCONNECTED。</summary>
    public static WakerOptions None { get; } = new(JsonValue.Create("none"));

    /// <summary>执行 program args…（不经 shell），唤醒请求以一行 JSON 写入其 stdin。</summary>
    public static WakerOptions Exec(string program, params string[] args)
    {
        if (string.IsNullOrEmpty(program)) throw new ArgumentException("program 不能为空", nameof(program));
        var argv = new JsonArray((JsonNode?)program);
        foreach (var a in args) argv.Add(a);
        return new WakerOptions(new JsonObject { ["exec"] = argv });
    }

    internal JsonNode ToJson() => _json.DeepClone();
}

public class HubException : Exception
{
    public HubException(HubStatus status, string message) : base(message) => Status = status;
    public HubStatus Status { get; }
}

/// <summary>
/// Hub 配置，序列化为 <c>am_hub_start</c> 的 config JSON（字段含义见 app_mcp_hub.h）。
/// 未设置（null）的字段使用库的默认值。
/// </summary>
public sealed class HubOptions
{
    /// <summary>App 连接服务监听地址（默认 "127.0.0.1:7717"；端口 0 随机）。</summary>
    public string? WsAddress { get; set; }

    /// <summary>为 true 时不开 App 连接服务（config 中 wsAddr = null），忽略 <see cref="WsAddress"/>。</summary>
    public bool DisableWebSocket { get; set; }

    /// <summary>
    /// 本地 IPC 端点（原生 App 默认连接这里，spec/protocol.md 1.2）："unix:&lt;绝对路径&gt;" 或
    /// "pipe:\\.\pipe\&lt;名称&gt;"；null 时为平台默认端点（Windows 为 \\.\pipe\app-mcp-&lt;用户 SID&gt;）。
    /// </summary>
    public string? IpcEndpoint { get; set; }

    /// <summary>为 true 时不开本地 IPC 服务（config 中 ipcEndpoint = null），忽略 <see cref="IpcEndpoint"/>。</summary>
    public bool DisableIpc { get; set; }

    /// <summary>静态清单对象（spec/manifest.md）。</summary>
    public IList<JsonNode> Manifests { get; } = new List<JsonNode>();
    public IList<string> ManifestFiles { get; } = new List<string>();
    public string? ManifestDir { get; set; }
    public IList<string> AllowOrigins { get; } = new List<string>();

    public TimeSpan? PingInterval { get; set; }
    public TimeSpan? IdleTimeout { get; set; }
    public TimeSpan? HiddenIdleTimeout { get; set; }
    public TimeSpan? InvokeTimeout { get; set; }
    public TimeSpan? ResponseTimeout { get; set; }
    public TimeSpan? ListChangedDebounce { get; set; }
    public TimeSpan? PairingTimeout { get; set; }

    // ---- 生命周期（spec/hub-api.md 3.5）----

    /// <summary>调用实例完成后发送的租约时长（默认 60 秒）；<see cref="TimeSpan.Zero"/> 关闭租约。</summary>
    public TimeSpan? LeaseTtl { get; set; }
    /// <summary>唤醒后等待 App 回连的上限（默认 15 秒），超时 → APP_NOT_RESPONDING。</summary>
    public TimeSpan? WakeTimeout { get; set; }
    /// <summary>唤醒令牌有效期（默认 60 秒）。</summary>
    public TimeSpan? WakeTokenTtl { get; set; }
    /// <summary>休眠记录保留时长（默认 24 小时），过期后不再列出其工具。</summary>
    public TimeSpan? DormantTtl { get; set; }
    /// <summary>同一 appId 以新实例 ID 连接时移除其休眠记录（默认 true）。</summary>
    public bool? DormantReplacedByNewInstance { get; set; }
    /// <summary>App 未运行且清单无显式 wake 时，是否由清单 launch 推导唤醒方式（默认 false）。</summary>
    public bool? WakeFromLaunch { get; set; }
    /// <summary>唤醒器（默认 <see cref="WakerOptions.System"/>）。</summary>
    public WakerOptions? Waker { get; set; }

    // ---- 渐进暴露（spec/hub-api.md 3.7）----

    /// <summary>工具暴露方式（默认 <see cref="AppMcp.Hub.ToolExposure.Auto"/>）。</summary>
    public ToolExposure? ToolExposure { get; set; }
    /// <summary>Auto 的阈值：App 与上游工具总数超过此值时渐进暴露（默认 40）。</summary>
    public int? ToolExposureThreshold { get; set; }

    /// <summary>上游 MCP 服务器（名称 → 启动方式）。</summary>
    public IDictionary<string, UpstreamOptions> Upstreams { get; } = new Dictionary<string, UpstreamOptions>();

    /// <summary>风险不低于此等级的调用需要审批（见 <see cref="AppMcpHub.ApprovalHandler"/>）；null = 不审批。</summary>
    public HubRisk? RequireApprovalAtOrAbove { get; set; }

    /// <summary>审批等待上限；null 用库的默认值。</summary>
    public TimeSpan? ApprovalTimeout { get; set; }

    /// <summary>tokio 工作线程数（默认 2）。</summary>
    public int? WorkerThreads { get; set; }

    /// <summary>事件、审批、配对 handler 使用的调度器。不设置时取 <see cref="AppMcpHub.Start(HubOptions)"/>
    /// 调用时的 <see cref="SynchronizationContext.Current"/>；显式设为 null 表示线程池。</summary>
    public SynchronizationContext? Dispatcher
    {
        get => _dispatcher;
        set
        {
            _dispatcher = value;
            DispatcherSet = true;
        }
    }

    private SynchronizationContext? _dispatcher;
    internal bool DispatcherSet { get; private set; }

    /// <summary>生成 config JSON。</summary>
    public string ToConfigJson()
    {
        var o = new JsonObject();
        if (DisableWebSocket) o["wsAddr"] = null;
        else if (WsAddress is not null) o["wsAddr"] = WsAddress;
        if (DisableIpc) o["ipcEndpoint"] = null;
        else if (IpcEndpoint is not null) o["ipcEndpoint"] = IpcEndpoint;
        if (Manifests.Count > 0) o["manifests"] = new JsonArray(Manifests.Select(m => m.DeepClone()).ToArray());
        if (ManifestFiles.Count > 0) o["manifestFiles"] = new JsonArray(ManifestFiles.Select(f => (JsonNode?)f).ToArray());
        if (ManifestDir is not null) o["manifestDir"] = ManifestDir;
        if (AllowOrigins.Count > 0) o["allowOrigins"] = new JsonArray(AllowOrigins.Select(f => (JsonNode?)f).ToArray());
        AddMs(o, "pingIntervalMs", PingInterval);
        AddMs(o, "idleTimeoutMs", IdleTimeout);
        AddMs(o, "hiddenIdleTimeoutMs", HiddenIdleTimeout);
        AddMs(o, "invokeTimeoutMs", InvokeTimeout);
        AddMs(o, "responseTimeoutMs", ResponseTimeout);
        AddMs(o, "listChangedDebounceMs", ListChangedDebounce);
        AddMs(o, "pairingTimeoutMs", PairingTimeout);
        AddMs(o, "leaseTtlMs", LeaseTtl);
        AddMs(o, "wakeTimeoutMs", WakeTimeout);
        AddMs(o, "wakeTokenTtlMs", WakeTokenTtl);
        AddMs(o, "dormantTtlMs", DormantTtl);
        if (DormantReplacedByNewInstance is { } drn) o["dormantReplacedByNewInstance"] = drn;
        if (WakeFromLaunch is { } wfl) o["wakeFromLaunch"] = wfl;
        if (Waker is { } wk) o["waker"] = wk.ToJson();
        if (ToolExposure is { } te) o["toolExposure"] = te.ToString().ToLowerInvariant();
        if (ToolExposureThreshold is { } tt)
        {
            if (tt < 0) throw new ArgumentOutOfRangeException(nameof(ToolExposureThreshold), "阈值不能为负数");
            o["toolExposureThreshold"] = tt;
        }
        if (Upstreams.Count > 0)
        {
            var ups = new JsonObject();
            foreach (var (name, u) in Upstreams) ups[name] = u.ToJson();
            o["upstreams"] = ups;
        }
        if (RequireApprovalAtOrAbove is not null || ApprovalTimeout is not null)
        {
            var a = new JsonObject();
            if (RequireApprovalAtOrAbove is { } r) a["requireAtOrAbove"] = r.ToProtocolString();
            AddMs(a, "timeout", ApprovalTimeout);
            o["approval"] = a;
        }
        if (WorkerThreads is { } w) o["workerThreads"] = w;
        return o.ToJsonString();
    }

    private static void AddMs(JsonObject o, string key, TimeSpan? value)
    {
        if (value is { } v)
        {
            if (v < TimeSpan.Zero) throw new ArgumentOutOfRangeException(key, "时长不能为负数");
            o[key] = (ulong)v.TotalMilliseconds;
        }
    }
}

/// <summary>上游 MCP 服务器（stdio 子进程）。</summary>
public sealed class UpstreamOptions
{
    public required string Command { get; init; }
    public IList<string> Args { get; init; } = new List<string>();
    public IDictionary<string, string> Env { get; init; } = new Dictionary<string, string>();

    internal JsonObject ToJson()
    {
        var o = new JsonObject { ["command"] = Command };
        if (Args.Count > 0) o["args"] = new JsonArray(Args.Select(a => (JsonNode?)a).ToArray());
        if (Env.Count > 0)
        {
            var env = new JsonObject();
            foreach (var (k, v) in Env) env[k] = v;
            o["env"] = env;
        }
        return o;
    }
}

/// <summary>工具过滤条件（ToolFilter）。</summary>
public sealed class ToolFilter
{
    /// <summary>只列这些 App；null = 全部。</summary>
    public IReadOnlyList<string>? Apps { get; init; }
    public HubRisk? MaxRisk { get; init; }
    public bool OnlyAvailable { get; init; }
    /// <summary>是否包含内置工具 apps.list / apps.select / apps.overview（渐进暴露生效时另有 apps.tools；默认 true）。</summary>
    public bool IncludeBuiltin { get; init; } = true;
    /// <summary>厂商会话 ID（null = 默认会话）。渐进暴露生效且 <see cref="Apps"/> 为 null 时，
    /// 只保留该会话已展开 / 调用过 / 选定了实例的 App 的工具。</summary>
    public string? Session { get; init; }

    internal string ToJson()
    {
        var o = new JsonObject
        {
            ["onlyAvailable"] = OnlyAvailable,
            ["includeBuiltin"] = IncludeBuiltin,
        };
        if (Apps is not null) o["apps"] = new JsonArray(Apps.Select(a => (JsonNode?)a).ToArray());
        if (MaxRisk is { } r) o["maxRisk"] = r.ToProtocolString();
        if (Session is not null) o["session"] = Session;
        return o.ToJsonString();
    }
}

/// <summary>工具调用请求（CallRequest）。</summary>
public sealed class CallRequest
{
    public CallRequest(string name, object? arguments = null)
    {
        Name = name;
        Arguments = arguments;
    }

    /// <summary>全名 &lt;appId&gt;.&lt;tool&gt;（内置工具为 apps.list 等）。</summary>
    public string Name { get; }

    /// <summary>参数对象（JsonNode / JsonElement / 可序列化对象）；null 视为 {}。</summary>
    public object? Arguments { get; init; }

    public string? InstanceId { get; init; }
    public TimeSpan? Timeout { get; init; }
    /// <summary>不设置时自动生成（见 <see cref="CallOutcome.CallId"/>）。</summary>
    public string? CallId { get; init; }
    /// <summary>厂商会话 ID；null = 默认会话。</summary>
    public string? Session { get; init; }

    internal string ToJson(JsonSerializerOptions options)
    {
        var o = new JsonObject
        {
            ["name"] = Name,
            ["arguments"] = Arguments is null ? new JsonObject() : JsonSerializer.SerializeToNode(Arguments, Arguments.GetType(), options),
        };
        if (InstanceId is not null) o["instanceId"] = InstanceId;
        if (Timeout is { } t) o["timeout"] = (ulong)Math.Max(0, t.TotalMilliseconds);
        if (CallId is not null) o["callId"] = CallId;
        if (Session is not null) o["session"] = Session;
        return o.ToJsonString();
    }
}

/// <summary>调用出错信息（{kind, message, details?}）。</summary>
public sealed record HubError(string Kind, string Message, JsonElement? Details);

/// <summary>工具调用结果（CallOutcome）。</summary>
public sealed class CallOutcome
{
    internal CallOutcome(JsonElement json)
    {
        Json = json;
        CallId = json.TryGetProperty("callId", out var id) ? id.GetString() ?? string.Empty : string.Empty;
        if (json.TryGetProperty("result", out var result))
        {
            if (result.TryGetProperty("ok", out var ok)) Data = ok;
            else if (result.TryGetProperty("error", out var err)) Error = ParseError(err);
        }
        if (json.TryGetProperty("stateHints", out var hints) && hints.ValueKind == JsonValueKind.Array)
        {
            StateHints = hints.EnumerateArray().Select(h => h.GetString() ?? string.Empty).ToArray();
        }
        if (json.TryGetProperty("instanceId", out var inst) && inst.ValueKind == JsonValueKind.String) InstanceId = inst.GetString();
        if (json.TryGetProperty("overview", out var ov) && ov.ValueKind == JsonValueKind.Object) Overview = ov;
    }

    public string CallId { get; }
    public bool IsSuccess => Error is null;
    /// <summary>成功时的结果数据。</summary>
    public JsonElement? Data { get; }
    public HubError? Error { get; }
    public IReadOnlyList<string> StateHints { get; } = [];
    public string? InstanceId { get; }
    /// <summary>本会话首次调用该 App 时附带的总览（AppOverviewInfo）。</summary>
    public JsonElement? Overview { get; }
    /// <summary>原始 CallOutcome JSON。</summary>
    public JsonElement Json { get; }

    internal static HubError ParseError(JsonElement err) => new(
        err.TryGetProperty("kind", out var k) ? k.GetString() ?? "INTERNAL" : "INTERNAL",
        err.TryGetProperty("message", out var m) ? m.GetString() ?? string.Empty : string.Empty,
        err.TryGetProperty("details", out var d) ? d : null);
}

/// <summary>资源读取结果。</summary>
public sealed record ResourceContents(string Uri, string? MimeType, string? Text, string? Blob);

/// <summary>读取资源失败。</summary>
public class HubCallException(HubError error) : Exception($"{error.Kind}: {error.Message}")
{
    public HubError Error { get; } = error;
}

/// <summary>Hub 事件类型（<see cref="HubEventArgs.Type"/> 的取值）。未来可能新增，调用方应忽略不认识的类型。</summary>
public static class HubEventTypes
{
    public const string AppConnected = "appConnected";
    public const string AppDisconnected = "appDisconnected";
    public const string ToolsChanged = "toolsChanged";
    public const string ResourcesChanged = "resourcesChanged";
    public const string ResourceUpdated = "resourceUpdated";
    public const string VisibilityChanged = "visibilityChanged";
    public const string UpstreamState = "upstreamState";
    /// <summary>实例进入休眠（工具仍列出，调用时唤醒；不另发 toolsChanged）。含 appId、instanceId。</summary>
    public const string AppDormant = "appDormant";
    /// <summary>Hub 正在唤醒 App。含 appId、instanceId（null = 冷启动）。</summary>
    public const string AppWaking = "appWaking";
    /// <summary>事件回调处理过慢导致丢失，应重新查询全量状态。</summary>
    public const string Lagged = "lagged";
}

/// <summary>工具可用性（HubTool.availability 的取值）。</summary>
public static class HubAvailability
{
    public const string Available = "available";
    public const string Disconnected = "disconnected";
    public const string NotRegistered = "notRegistered";
    /// <summary>只由休眠实例提供；调用时 Hub 先唤醒再派发。</summary>
    public const string Dormant = "dormant";
}

/// <summary>实例信息（InstanceInfo）。</summary>
public sealed record InstanceInfo(
    string InstanceId,
    string ClientKind,
    string Visibility,
    bool Focused,
    ulong LastActiveMs,
    string? Title);

/// <summary>App 信息（AppInfo）。</summary>
public sealed record AppInfo(
    string AppId,
    string Name,
    string Kind,
    string? Summary,
    bool Connected,
    IReadOnlyList<InstanceInfo> Instances,
    string? SelectedInstance,
    IReadOnlyList<InstanceInfo>? DormantInstances)
{
    /// <summary>休眠中的实例（按休眠时间排列）；旧版本库不返回时为空。</summary>
    public IReadOnlyList<InstanceInfo> DormantInstances { get; init; } = DormantInstances ?? [];

    /// <summary>无已连接实例、但有休眠实例（调用其工具时会唤醒）。</summary>
    public bool IsDormant => !Connected && DormantInstances.Count > 0;
}

/// <summary>工具信息（HubTool）。</summary>
public sealed record HubToolInfo(
    string Name,
    string AppId,
    string Tool,
    string? Title,
    string Description,
    JsonElement InputSchema,
    string Risk,
    string Activation,
    string Availability)
{
    public bool IsDormant => Availability == HubAvailability.Dormant;
}

/// <summary>唤醒请求（WakeRequest，spec/hub-api.md 3.5），交给 <see cref="AppMcpHub.Waker"/>。</summary>
/// <param name="AppId">要唤醒的 App。</param>
/// <param name="InstanceId">被唤醒的休眠实例；null = App 未运行，按清单冷启动。</param>
/// <param name="Descriptor">唤醒描述：{kind, target?, background}。</param>
/// <param name="Token">一次性唤醒令牌（32 位十六进制）。</param>
/// <param name="ActivationArg">通用激活参数 <c>app-mcp-wake:&lt;token&gt;</c>，App 端 SDK 的 HandleWake 可识别。</param>
public sealed record WakeRequest(
    string AppId,
    string? InstanceId,
    WakeTarget Descriptor,
    string Token,
    string ActivationArg);

/// <summary>唤醒描述。<see cref="Kind"/> 为 uri / aumid / apple-event / dbus / android-intent / web-url。</summary>
public sealed record WakeTarget(string Kind, string? Target, bool Background);

/// <summary>在 <see cref="AppMcpHub.Waker"/> 中抛出，以指定错误类别结束调用（其他异常按 LAUNCH_FAILED）。</summary>
public class WakeFailedException(string kind, string message) : Exception(message)
{
    /// <summary>协议错误类别，如 LAUNCH_FAILED、APP_NOT_INSTALLED。</summary>
    public string Kind { get; } = kind;
}

/// <summary>Hub 事件。<see cref="Type"/> 为 appConnected、toolsChanged 等（spec/hub-api.md 3.1）；
/// 回调处理过慢导致事件丢失时为 "lagged"，此时应重新查询全量状态。
/// 未来新增的事件类型原样透传，调用方应忽略不认识的类型。</summary>
public sealed class HubEventArgs(string type, JsonElement json) : EventArgs
{
    public string Type { get; } = type;
    /// <summary>完整事件 JSON（含 type）。</summary>
    public JsonElement Json { get; } = json;

    public string? AppId => Str("appId");
    public string? InstanceId => Str("instanceId");
    public string? Uri => Str("uri");

    private string? Str(string name) =>
        Json.TryGetProperty(name, out var v) && v.ValueKind == JsonValueKind.String ? v.GetString() : null;
}

/// <summary>调用审批请求（ApprovalRequest）。</summary>
public sealed record ApprovalRequest(
    string CallId,
    string AppId,
    string AppName,
    string Tool,
    string? Title,
    string Description,
    string Risk,
    JsonElement Arguments,
    string? Session);

/// <summary>App 配对请求（PairingRequest）。</summary>
public sealed record PairingRequest(
    string AppId,
    string AppName,
    string? Origin,
    string ClientKind,
    string InstanceId);
