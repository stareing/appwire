// 调用：过滤、请求、结果、错误与事件类型名（spec/hub-api.md 3.2）。
using System.Text.Json;
using System.Text.Json.Nodes;
using System.Text.Json.Serialization;

namespace AppMcp.Hub;

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
    /// <summary>Agent 的幂等键（1..=256 个字符），原样转交 App（spec/hub-api.md 3.15）；不合法时结果为 INVALID_INPUT。</summary>
    public string? IdempotencyKey { get; init; }
    /// <summary>调用优先级（第 16 项 P6，spec/hub-api.md 3.15）：原样转交 App，App 的调用队列先按它、再按到达顺序调度；null = Normal。</summary>
    public CallPriority? Priority { get; init; }
    /// <summary>true：不查只读结果缓存、照常调用并以新结果覆盖（spec/hub-api.md 3.20）。默认 false。</summary>
    public bool CacheBypass { get; init; }

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
        if (IdempotencyKey is not null) o["idempotencyKey"] = IdempotencyKey;
        if (Priority is { } p) o["priority"] = p.ToProtocolString();
        if (CacheBypass) o["cacheBypass"] = true;
        return o.ToJsonString();
    }
}

/// <summary>调用优先级（第 16 项 P6，spec/hub-api.md 3.15）：交互（用户在等结果）&gt; 普通 &gt; 后台。</summary>
public enum CallPriority
{
    /// <summary>用户在场等结果。</summary>
    Interactive,
    /// <summary>缺省。</summary>
    Normal,
    /// <summary>定时、批量等后台作业。</summary>
    Background,
}

public static class CallPriorityExtensions
{
    /// <summary>请求 JSON 中的取值（"interactive" / "normal" / "background"）。</summary>
    public static string ToProtocolString(this CallPriority priority) => priority switch
    {
        CallPriority.Interactive => "interactive",
        CallPriority.Background => "background",
        _ => "normal",
    };
}

/// <summary>调用出错信息（{kind, message, details?}）。Kind 为协议错误类别，如 TOOL_NOT_FOUND；Hub 的资源保护另有
/// RATE_LIMITED（Details：retryAfterMs、scope "tool"/"app"、perMinute、burst、appId、tool）与
/// PAYLOAD_TOO_LARGE（Details：part "arguments"/"result"/"resource"、sizeBytes、limitBytes）。</summary>
public sealed record HubError(string Kind, string Message, JsonElement? Details)
{
    /// <summary>Hub 侧限流。</summary>
    public const string RateLimited = "RATE_LIMITED";
    /// <summary>参数、结果或资源内容超过 Hub 的大小上限。</summary>
    public const string PayloadTooLarge = "PAYLOAD_TOO_LARGE";
    /// <summary>被本机的 Deny 策略规则拒绝，操作未执行；重试不会改变结果。Details：ruleId、hook（"call"/"wake"）、appId、tool。</summary>
    public const string PolicyDenied = "POLICY_DENIED";
    /// <summary>需要用户本人操作（登录、授权、切到前台、在 App 内确认）后才能继续。Details：reason?、uri?。</summary>
    public const string UserActionRequired = "USER_ACTION_REQUIRED";
    /// <summary>导航没有完成（不支持 / 出错 / 超时 / 导航后工具未出现，spec/protocol.md 3.4）。Details：reason、appId、page。</summary>
    public const string NavigationFailed = "NAVIGATION_FAILED";
    /// <summary>导航被拒绝（App 拒绝或页面不可由 Agent 导航）。Details：reason（"app" / "not-navigable"）、appId、page。</summary>
    public const string NavigationDenied = "NAVIGATION_DENIED";
    /// <summary>对象锁冲突（spec/hub-api.md 3.6「对象锁」，只由 Hub 产生）：App 正被其他调用方以 apps.lock 锁定，写调用未转发；
    /// 或要加的锁已被他人持有。Details：appId、key?、holder（"agent:&lt;名&gt;" / "local" / "api"）、retryAfterMs。</summary>
    public const string Locked = "LOCKED";
}

/// <summary>App 声明的调用结果业务状态（spec/protocol.md 3.2）。</summary>
public enum HubResultStatus
{
    /// <summary>已完成（缺省）。</summary>
    Done,
    /// <summary>已受理、尚未完成；后续状态见 <see cref="CallOutcome.StateResource"/>。</summary>
    Pending,
    /// <summary>只完成了一部分，说明见 <see cref="CallOutcome.Summary"/>。</summary>
    Partial,
    /// <summary>没有做任何改动。</summary>
    Noop,
}

/// <summary>标准 MCP 工具注解（HubTool.annotations 等）；为 null 的字段未声明。</summary>
public sealed record HubToolAnnotations(
    string? Title,
    bool? ReadOnlyHint,
    bool? DestructiveHint,
    bool? IdempotentHint,
    bool? OpenWorldHint);

/// <summary>MCP 内容注解（结果 / 资源内容的标注）。Audience 的元素为 "user" / "assistant"。</summary>
public sealed record HubContentAnnotations(IReadOnlyList<string>? Audience, double? Priority, string? LastModified);

/// <summary>调用进度（spec/hub-api.md 3.12）：App 报告、经 Hub 合并且递增。Total 未知时为 null；Message 最长 200 字符。</summary>
public sealed record CallProgress(string CallId, double Progress, double? Total, string? Message);

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
        if (json.TryGetProperty("status", out var st) && st.ValueKind == JsonValueKind.String)
        {
            Status = st.GetString() switch
            {
                "pending" => HubResultStatus.Pending,
                "partial" => HubResultStatus.Partial,
                "noop" => HubResultStatus.Noop,
                _ => HubResultStatus.Done,
            };
        }
        if (json.TryGetProperty("stateResource", out var sr) && sr.ValueKind == JsonValueKind.String) StateResource = sr.GetString();
        if (json.TryGetProperty("summary", out var sum) && sum.ValueKind == JsonValueKind.String) Summary = sum.GetString();
        if (json.TryGetProperty("annotations", out var ann) && ann.ValueKind == JsonValueKind.Object)
        {
            Annotations = ann.Deserialize<HubContentAnnotations>(AppMcpHub.WireOptions);
        }
        if (json.TryGetProperty("routedTo", out var rt) && rt.ValueKind == JsonValueKind.String) RoutedTo = rt.GetString();
        if (json.TryGetProperty("durationMs", out var dur) && dur.ValueKind == JsonValueKind.Number && dur.TryGetInt64(out var ms)) DurationMs = ms;
        if (json.TryGetProperty("woke", out var woke) && woke.ValueKind is JsonValueKind.True or JsonValueKind.False) Woke = woke.GetBoolean();
        if (json.TryGetProperty("cachedAgeMs", out var age) && age.ValueKind == JsonValueKind.Number && age.TryGetInt64(out var ageMs)) CachedAgeMs = ageMs;
        if (json.TryGetProperty("undo", out var undo) && undo.ValueKind == JsonValueKind.Object) Undo = undo.Deserialize<UndoOfferInfo>(AppMcpHub.WireOptions);
        if (json.TryGetProperty("undoOf", out var undoOf) && undoOf.ValueKind == JsonValueKind.String) UndoOf = undoOf.GetString();
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
    /// <summary>App 声明的业务状态（缺省 <see cref="HubResultStatus.Done"/>；未知值按 Done）。</summary>
    public HubResultStatus Status { get; } = HubResultStatus.Done;
    /// <summary>Pending 时可读取后续状态的资源 URI（app-mcp://&lt;appId&gt;/&lt;名&gt;）。</summary>
    public string? StateResource { get; }
    /// <summary>App 给出的一句结论。</summary>
    public string? Summary { get; }
    /// <summary>App 对结果内容的标注（MCP 内容注解），原样。</summary>
    public HubContentAnnotations? Annotations { get; }
    /// <summary>App 在后台、Hub 改调了 view 工具声明的后台替代时为实际调用的工具全名（spec/hub-api.md 3.14）；否则为 null。</summary>
    public string? RoutedTo { get; }
    /// <summary>Hub 收到调用到得出结果的毫秒数（含审批、唤醒与等待 App；spec/hub-api.md 3.15）。旧 Hub 未给出时为 0。</summary>
    public long DurationMs { get; }
    /// <summary>本次 App 工具调用是否经历了唤醒（调用时目标未连接，唤醒 / 按名激活回连后才送达；spec/hub-api.md 3.15）。
    /// 内置工具与上游工具恒为 false；旧 Hub 未给出时为 false。</summary>
    public bool Woke { get; }
    /// <summary>结果来自只读结果缓存（未转发给 App）时距 App 产出的毫秒数；未命中或旧 Hub 为 null（spec/hub-api.md 3.20）。</summary>
    public long? CachedAgeMs { get; }
    /// <summary>本次调用已登记撤销（App 结果带合法撤销信息，spec/hub-api.md 3.23）：可用 apps.undo 撤销；未登记或旧 Hub 为 null。</summary>
    public UndoOfferInfo? Undo { get; }
    /// <summary>apps.undo 的结果：被撤销调用的 callId；其他调用或旧 Hub 为 null。</summary>
    public string? UndoOf { get; }
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
    /// <summary>SDK 上报了此前遇到的连接问题（app/diagnostic）。含 appId、instanceId、code、message、count。</summary>
    public const string AppDiagnostic = "appDiagnostic";
    /// <summary>App 发出的事件（已去重与校验，不论有无订阅）。字段同 <see cref="HubAppEvent"/>：id、appId、instanceId、name、payload?、at。
    /// 处理过慢时可能 lagged；需要不丢的用 <see cref="AppMcpHub.SetEventHandler"/>。</summary>
    public const string AppEvent = "appEvent";
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

/// <summary>App 工具对界面的依赖（HubTool.surface 的取值，spec/protocol.md 3.4）。</summary>
public static class HubToolSurface
{
    /// <summary>不依赖界面（未声明即此值）。</summary>
    public const string App = "app";
    /// <summary>只在所在界面可见且处于最上层时注册。</summary>
    public const string View = "view";
}
