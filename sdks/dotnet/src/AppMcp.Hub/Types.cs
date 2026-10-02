using System.Text.Json;
using System.Text.Json.Nodes;
using System.Text.Json.Serialization;

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
    /// <summary>请求的能力未编入本 Hub 库（cargo feature 关闭）；错误说明含缺少的 feature 名。此前同类错误为 <see cref="Io"/>。</summary>
    Unsupported = 10,
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

/// <summary>结果与工具 outputSchema 不符时 Hub 的处理（spec/hub-api.md 3.11）。</summary>
[JsonConverter(typeof(JsonStringEnumConverter<OutputValidation>))]
public enum OutputValidation
{
    /// <summary>不校验。</summary>
    Off,
    /// <summary>校验，不符时只记日志（默认）。</summary>
    Log,
    /// <summary>校验，不符时调用以 HANDLER_ERROR 结束。</summary>
    Reject,
}

/// <summary>
/// 资源保护（spec/hub-api.md 3.11，JSON 形式即 LimitOverrides）：按（App, 工具）与按 App 两级令牌桶限流，超出 → RATE_LIMITED；
/// 参数 / 结果 / 资源超过字节上限 → PAYLOAD_TOO_LARGE（不截断）。为 null 的字段取默认值（工具 120/分钟、突发 30；
/// App 600/分钟、突发 60；参数 1 MiB、结果 4 MiB、资源 4 MiB）。*PerMinute = 0 不限流，*Bytes = 0 不限大小；
/// 限流时 *Burst = 0 启动失败（InvalidConfig）。也用于 <see cref="HubStatusInfo.Limits"/>（全部字段给出）。
/// </summary>
public sealed class HubLimits
{
    public uint? ToolRatePerMinute { get; set; }
    public uint? ToolRateBurst { get; set; }
    public uint? AppRatePerMinute { get; set; }
    public uint? AppRateBurst { get; set; }
    public ulong? MaxArgumentsBytes { get; set; }
    public ulong? MaxResultBytes { get; set; }
    public ulong? MaxResourceBytes { get; set; }

    internal JsonObject ToJson()
    {
        var o = new JsonObject();
        if (ToolRatePerMinute is { } a) o["toolRatePerMinute"] = a;
        if (ToolRateBurst is { } b) o["toolRateBurst"] = b;
        if (AppRatePerMinute is { } c) o["appRatePerMinute"] = c;
        if (AppRateBurst is { } d) o["appRateBurst"] = d;
        if (MaxArgumentsBytes is { } e) o["maxArgumentsBytes"] = e;
        if (MaxResultBytes is { } f) o["maxResultBytes"] = f;
        if (MaxResourceBytes is { } g) o["maxResourceBytes"] = g;
        return o;
    }
}

/// <summary>策略规则的动作（spec/hub-api.md 3.13）。</summary>
[JsonConverter(typeof(JsonStringEnumConverter<HubPolicyAction>))]
public enum HubPolicyAction
{
    /// <summary>工具（或整个 App）不出现在任何列表中，调用为 TOOL_NOT_FOUND。</summary>
    Hide,
    /// <summary>可见，在 <see cref="HubPolicyRule.Hooks"/> 指定的执行点以 POLICY_DENIED 拒绝。</summary>
    Deny,
}

/// <summary>策略执行点。规则的 Hooks 只能写 Call / Wake（List 由 Hide 隐式使用，Handle 尚未实现，写了校验报错）。</summary>
[JsonConverter(typeof(JsonStringEnumConverter<HubPolicyHook>))]
public enum HubPolicyHook { List, Call, Wake, Handle }

/// <summary>按 App 声明的 MCP 注解匹配：给出的每一项都与工具注解相等才命中（工具未声明该项时不命中）。至少给出一项。</summary>
public sealed class HubAnnotationMatch
{
    public bool? ReadOnlyHint { get; set; }
    public bool? DestructiveHint { get; set; }
    public bool? IdempotentHint { get; set; }
    public bool? OpenWorldHint { get; set; }

    internal JsonObject ToJson()
    {
        var o = new JsonObject();
        if (ReadOnlyHint is { } a) o["readOnlyHint"] = a;
        if (DestructiveHint is { } b) o["destructiveHint"] = b;
        if (IdempotentHint is { } c) o["idempotentHint"] = c;
        if (OpenWorldHint is { } d) o["openWorldHint"] = d;
        return o;
    }
}

/// <summary>一条策略规则。Tool 与 Annotations 都为 null 时作用于整个 App。</summary>
public sealed class HubPolicyRule
{
    /// <summary>规则标识：[A-Za-z0-9_.-]{1,64}，在规则集中唯一；POLICY_DENIED 的 Details.ruleId。</summary>
    public required string Id { get; set; }
    public required HubPolicyAction Action { get; set; }
    /// <summary>appId（或上游名）：精确名，或以 * 结尾的前缀；"*" 匹配全部。</summary>
    public required string App { get; set; }
    /// <summary>工具局部名（不含 appId），规则同 App。</summary>
    public string? Tool { get; set; }
    public HubAnnotationMatch? Annotations { get; set; }
    /// <summary>只用于 Deny：Call / Wake 的非空子集；null 为 [Call]。</summary>
    public IList<HubPolicyHook>? Hooks { get; set; }

    internal static string HookString(HubPolicyHook h) => h.ToString().ToLowerInvariant();

    internal JsonObject ToJson()
    {
        var o = new JsonObject
        {
            ["id"] = Id,
            ["action"] = Action.ToString().ToLowerInvariant(),
            ["app"] = App,
        };
        if (Tool is not null) o["tool"] = Tool;
        if (Annotations is { } m) o["annotations"] = m.ToJson();
        if (Hooks is { } hooks) o["hooks"] = new JsonArray(hooks.Select(h => (JsonNode?)HookString(h)).ToArray());
        return o;
    }
}

/// <summary>
/// 策略规则集（spec/hub-api.md 3.13）：按顺序匹配，Deny 取第一条命中的规则；空规则集 = 不做任何限制（默认）。
/// 用于 <see cref="HubOptions.Policy"/> 与 <see cref="AppMcpHub.SetPolicy"/>。
/// </summary>
public sealed class HubPolicy
{
    public IList<HubPolicyRule> Rules { get; init; } = new List<HubPolicyRule>();

    /// <summary>JSON 形式 {"rules":[{"id","action","app","tool"?,"annotations"?,"hooks"?}]}。</summary>
    public string ToJsonString() => ToJson().ToJsonString();

    internal JsonObject ToJson() =>
        new() { ["rules"] = new JsonArray(Rules.Select(r => (JsonNode?)r.ToJson()).ToArray()) };
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
    /// <summary>
    /// HTTP 监听地址：/app（App 的 WebSocket 连接）、/healthz，<see cref="McpHttp"/> 时另有 /mcp（spec/protocol.md 1.3）。
    /// null 时为 "127.0.0.1:7717"（被占用时依次尝试 7737、7757）；显式给出时只绑定该地址；端口 0 随机。
    /// </summary>
    public string? Listen { get; set; }

    /// <summary>为 true 时不开 HTTP 服务（config 中 listen = null），忽略 <see cref="Listen"/>。</summary>
    public bool DisableListen { get; set; }

    /// <summary>是否在 <see cref="Listen"/> 上提供 MCP Streamable HTTP（/mcp），默认 false。</summary>
    public bool McpHttp { get; set; }

    /// <summary>单实例锁与登记文件目录（&lt;RunDir&gt;/hub.lock、endpoints.json，spec/protocol.md 1.5、1.7）；null 时不参与。</summary>
    public string? RunDir { get; set; }

    /// <summary>
    /// 持久状态目录（spec/hub-api.md 3.5「持久化」）：休眠记录写到 &lt;StateDir&gt;/dormant/&lt;appId&gt;.json（原子写、仅当前用户可读），
    /// 启动时读回，重启前休眠的 App 仍可列出、可唤醒。null 时不读写任何文件。
    /// </summary>
    public string? StateDir { get; set; }

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
    /// <summary>进度转发的最小间隔（默认 250 毫秒，间隔内只保留最新一条），见 <see cref="AppMcpHub.CallAsync(CallRequest, IProgress{CallProgress}, CancellationToken)"/>。</summary>
    public TimeSpan? ProgressInterval { get; set; }

    // ---- 生命周期（spec/hub-api.md 3.5）----

    /// <summary>调用实例完成后发送的租约时长（默认 60 秒）；<see cref="TimeSpan.Zero"/> 关闭租约。</summary>
    public TimeSpan? LeaseTtl { get; set; }
    /// <summary>唤醒后等待 App 回连的上限（默认 15 秒），超时 → APP_NOT_RESPONDING。</summary>
    public TimeSpan? WakeTimeout { get; set; }
    /// <summary>导航等待上限（App 回复 + 目标工具注册，默认 5 秒，独立于 <see cref="WakeTimeout"/>；spec/hub-api.md 3.14 / 3.15），
    /// 超时 → NAVIGATION_FAILED。</summary>
    public TimeSpan? NavigateTimeout { get; set; }
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

    // ---- 功耗（spec/lifecycle.md 第 11–13 节）----

    /// <summary>每 App 每分钟最多实际发出的唤醒激活次数（默认 6）；0 不限。超出时调用以 LAUNCH_FAILED
    /// （Details.code = "WAKE_RATE_LIMITED"）结束。</summary>
    public int? WakeRateLimit { get; set; }
    /// <summary>回退到旧心跳：对所有 App 连接发 ping 并按无消息断开（默认 false）。</summary>
    public bool? LegacyHeartbeat { get; set; }
    /// <summary>自适应租约（默认见 <see cref="LeaseOptions"/>）。</summary>
    public LeaseOptions? Lease { get; set; }

    // ---- 资源保护与结果约定（spec/hub-api.md 3.11）----

    /// <summary>调用频率与数据大小上限（默认见 <see cref="HubLimits"/>）。</summary>
    public HubLimits? Limits { get; set; }
    /// <summary>结果与 outputSchema 不符时的处理（默认 <see cref="AppMcp.Hub.OutputValidation.Log"/>）。</summary>
    public OutputValidation? OutputValidation { get; set; }

    // ---- 策略挂点（spec/hub-api.md 3.13）----

    /// <summary>隐藏 / 拒绝规则；null = 无规则（行为不变）。规则不合法时启动失败（InvalidConfig）。
    /// 运行中用 <see cref="AppMcpHub.SetPolicy"/> 替换。</summary>
    public HubPolicy? Policy { get; set; }

    // ---- 渐进暴露（spec/hub-api.md 3.7）----

    /// <summary>工具暴露方式（默认 <see cref="AppMcp.Hub.ToolExposure.Auto"/>）。</summary>
    public ToolExposure? ToolExposure { get; set; }
    /// <summary>Auto 的阈值：App 与上游工具总数超过此值时渐进暴露（默认 40）。</summary>
    public int? ToolExposureThreshold { get; set; }

    // ---- 无会话 MCP 请求（spec/hub-api.md 3.6 / 3.7）----

    /// <summary>无会话调用方（principal:&lt;主体&gt;）的 Agent 任务在请求流空闲多久后回收（收回租约、清除选择），默认 10 分钟；
    /// <see cref="TimeSpan.Zero"/> 不因空闲回收。</summary>
    public TimeSpan? TaskIdleTtl { get; set; }
    /// <summary>无会话请求的工具暴露方式（默认 <see cref="AppMcp.Hub.ToolExposure.All"/>）；渐进时列表只含内置工具与全局选定实例的 App，
    /// 不随调用变化。</summary>
    public ToolExposure? StatelessToolExposure { get; set; }
    /// <summary>无会话请求的主体级 apps.select 选择的空闲有效期（默认 60 秒）；<see cref="TimeSpan.Zero"/> 不单独过期。</summary>
    public TimeSpan? PrincipalSelectTtl { get; set; }
    /// <summary>无会话请求的列表结果所带缓存提示 ttlMs（默认 5 秒）。</summary>
    public TimeSpan? StatelessListTtl { get; set; }

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
        if (DisableListen) o["listen"] = null;
        else if (Listen is not null) o["listen"] = Listen;
        if (McpHttp) o["mcpHttp"] = true;
        if (RunDir is not null) o["runDir"] = RunDir;
        if (StateDir is not null) o["stateDir"] = StateDir;
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
        AddMs(o, "progressIntervalMs", ProgressInterval);
        AddMs(o, "leaseTtlMs", LeaseTtl);
        AddMs(o, "wakeTimeoutMs", WakeTimeout);
        AddMs(o, "navigateTimeoutMs", NavigateTimeout);
        AddMs(o, "wakeTokenTtlMs", WakeTokenTtl);
        AddMs(o, "dormantTtlMs", DormantTtl);
        if (DormantReplacedByNewInstance is { } drn) o["dormantReplacedByNewInstance"] = drn;
        if (WakeFromLaunch is { } wfl) o["wakeFromLaunch"] = wfl;
        if (Waker is { } wk) o["waker"] = wk.ToJson();
        if (WakeRateLimit is { } wrl)
        {
            if (wrl < 0) throw new ArgumentOutOfRangeException(nameof(WakeRateLimit), "唤醒次数上限不能为负数");
            o["wakeRateLimit"] = wrl;
        }
        if (LegacyHeartbeat is { } lh) o["legacyHeartbeat"] = lh;
        if (Lease is { } lease) o["lease"] = lease.ToJson();
        if (Limits is { } limits) o["limits"] = limits.ToJson();
        if (OutputValidation is { } ov) o["outputValidation"] = ov.ToString().ToLowerInvariant();
        if (Policy is { } policy) o["policy"] = policy.ToJson();
        if (ToolExposure is { } te) o["toolExposure"] = te.ToString().ToLowerInvariant();
        if (ToolExposureThreshold is { } tt)
        {
            if (tt < 0) throw new ArgumentOutOfRangeException(nameof(ToolExposureThreshold), "阈值不能为负数");
            o["toolExposureThreshold"] = tt;
        }
        AddMs(o, "taskIdleTtlMs", TaskIdleTtl);
        if (StatelessToolExposure is { } ste) o["statelessToolExposure"] = ste.ToString().ToLowerInvariant();
        AddMs(o, "principalSelectTtlMs", PrincipalSelectTtl);
        AddMs(o, "statelessListTtlMs", StatelessListTtl);
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

    internal static void AddMs(JsonObject o, string key, TimeSpan? value)
    {
        if (value is { } v)
        {
            if (v < TimeSpan.Zero) throw new ArgumentOutOfRangeException(key, "时长不能为负数");
            o[key] = (ulong)v.TotalMilliseconds;
        }
    }
}

/// <summary>
/// 自适应租约策略（spec/lifecycle.md 第 13 节 B2）：租约 = 同一（会话, App）最近 <see cref="Window"/> 个调用间隔的
/// p90 + <see cref="Margin"/>，限制在 [<see cref="Min"/>, <see cref="Max"/>]；样本不足 3 个时用 <see cref="HubOptions.LeaseTtl"/>。
/// 为 null 的字段取默认值。Window = 0 或 Min &gt; Max 时启动失败（InvalidConfig）。
/// </summary>
public sealed class LeaseOptions
{
    /// <summary>是否按调用间隔自适应（默认 true）；false = 固定 LeaseTtl（4e 之前的行为）。</summary>
    public bool? Adaptive { get; set; }
    /// <summary>统计最近多少个间隔（默认 20）。</summary>
    public int? Window { get; set; }
    /// <summary>p90 之上的余量（默认 5 秒）。</summary>
    public TimeSpan? Margin { get; set; }
    /// <summary>下限（默认 5 秒）。</summary>
    public TimeSpan? Min { get; set; }
    /// <summary>上限（默认 60 秒）；超过它的调用间隔不计入统计。</summary>
    public TimeSpan? Max { get; set; }
    /// <summary>会话无请求多久后收回其默认租约（默认 30 秒）；<see cref="TimeSpan.Zero"/> 不收回。</summary>
    public TimeSpan? IdleRevoke { get; set; }

    internal JsonObject ToJson()
    {
        var o = new JsonObject();
        if (Adaptive is { } a) o["adaptive"] = a;
        if (Window is { } w)
        {
            if (w < 0) throw new ArgumentOutOfRangeException(nameof(Window), "窗口不能为负数");
            o["window"] = w;
        }
        HubOptions.AddMs(o, "marginMs", Margin);
        HubOptions.AddMs(o, "minMs", Min);
        HubOptions.AddMs(o, "maxMs", Max);
        HubOptions.AddMs(o, "idleRevokeMs", IdleRevoke);
        return o;
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
    /// <summary>Agent 的幂等键（1..=256 个字符），原样转交 App（spec/hub-api.md 3.15）；不合法时结果为 INVALID_INPUT。</summary>
    public string? IdempotencyKey { get; init; }

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
        return o.ToJsonString();
    }
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
}

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
    /// <summary>Agent 实际看到的 MCP 工具注解（声明优先、缺少的按 Risk 推导；上游为原样）；旧 Hub 为 null。</summary>
    public HubToolAnnotations? Annotations { get; init; }
    /// <summary>App 声明的结果 JSON Schema（MCP outputSchema）；未声明为 null。</summary>
    public JsonElement? OutputSchema { get; init; }
    /// <summary>App 工具的界面依赖（<see cref="HubToolSurface"/> 的取值）；内置与上游工具为 null。</summary>
    public string? Surface { get; init; }
    /// <summary>App 工具所在页面（spec/hub-api.md 3.14）；不属于页面时为 null。</summary>
    public string? Page { get; init; }
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
    string? Session)
{
    /// <summary>与 <see cref="HubToolInfo.Annotations"/> 相同；旧 Hub 为 null。</summary>
    public HubToolAnnotations? Annotations { get; init; }
    /// <summary>MCP 出口：发起调用的认证主体（取自传输层凭据，现在恒为 "local"）；经 Hub API 发起时为 null。</summary>
    public string? Principal { get; init; }
    /// <summary>MCP 出口：客户端自报的 clientInfo.name；经 Hub API 发起时为 null。自报、不可信，仅供显示，不得据此做授权决定。</summary>
    public string? ClientName { get; init; }
}

/// <summary>App 配对请求（PairingRequest）。</summary>
public sealed record PairingRequest(
    string AppId,
    string AppName,
    string? Origin,
    string ClientKind,
    string InstanceId);
