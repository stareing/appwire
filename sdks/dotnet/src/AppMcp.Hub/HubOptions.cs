// Hub 配置：HubOptions、租约与上游选项（spec/hub-api.md 3.1）。
using System.Text.Json;
using System.Text.Json.Nodes;
using System.Text.Json.Serialization;

namespace AppMcp.Hub;

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

    // ---- Agent 身份（spec/hub-api.md 3.6）----

    /// <summary>按 Agent 发的访问令牌；null = 不登记（所有请求为本机主体）。不合法时启动失败（InvalidConfig）。
    /// 运行中用 <see cref="AppMcpHub.SetAgents"/> 替换。</summary>
    public IList<HubAgentCredential>? Agents { get; set; }

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

    // ---- MCP 出口协议版本与通知（spec/hub-api.md 3.6）----

    /// <summary>协商的协议版本范围（默认 <see cref="AppMcp.Hub.McpProtocolMode.Auto"/>）。</summary>
    public McpProtocolMode? McpProtocolMode { get; set; }
    /// <summary>每个主体同时打开的 subscriptions/listen 流数上限（默认 16）；0 不提供 listen。</summary>
    public int? MaxListenStreams { get; set; }
    /// <summary>一个 listen 流接受的资源 URI 数上限（默认 256）。</summary>
    public int? MaxListenResources { get; set; }

    // ---- 任务句柄（spec/hub-api.md 3.6「任务句柄」）----

    /// <summary>每个主体同时存在的任务句柄数上限（默认 32，超出时 apps.task.begin 报 RATE_LIMITED）；0 不提供任务句柄。</summary>
    public int? MaxTaskHandles { get; set; }

    // ---- 对象锁（spec/hub-api.md 3.6「对象锁」）----

    /// <summary>每个持有者同时持有的对象锁数上限（默认 16，超出时 apps.lock 报 RATE_LIMITED）；0 不提供对象锁
    /// （apps.lock / apps.unlock 不列出，调用为 TOOL_NOT_FOUND）。</summary>
    public int? MaxLocks { get; set; }

    // ---- 事件信箱（spec/hub-api.md 3.17）----

    /// <summary>订阅数、信箱容量、保留时长与每订阅频率上限（默认见 <see cref="HubEventLimits"/>）。</summary>
    public HubEventLimits? EventLimits { get; set; }

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
        if (Agents is { } agents) o["agents"] = HubAgentCredential.ToJson(agents);
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
        if (McpProtocolMode is { } mode)
        {
            o["mcpProtocolMode"] = mode switch
            {
                AppMcp.Hub.McpProtocolMode.Auto => "auto",
                AppMcp.Hub.McpProtocolMode.LegacyOnly => "legacyOnly",
                _ => throw new ArgumentOutOfRangeException(nameof(McpProtocolMode)),
            };
        }
        AddCount(o, "maxListenStreams", MaxListenStreams, nameof(MaxListenStreams));
        AddCount(o, "maxListenResources", MaxListenResources, nameof(MaxListenResources));
        AddCount(o, "maxTaskHandles", MaxTaskHandles, nameof(MaxTaskHandles));
        AddCount(o, "maxLocks", MaxLocks, nameof(MaxLocks));
        if (EventLimits is { } el) o["eventLimits"] = el.ToJson();
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

    internal static void AddCount(JsonObject o, string key, int? value, string name)
    {
        if (value is { } v)
        {
            if (v < 0) throw new ArgumentOutOfRangeException(name, "上限不能为负数");
            o[key] = v;
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

/// <summary>事件信箱上限（spec/hub-api.md 3.17，配置 JSON 的 eventLimits）。为 null 的字段取默认值；负数在生成配置时抛
/// <see cref="ArgumentOutOfRangeException"/>。</summary>
public sealed class HubEventLimits
{
    /// <summary>每个订阅方最多的订阅数（默认 32）；超出时 apps.events.subscribe 报 RATE_LIMITED（details.scope = "events"）。</summary>
    public int? MaxSubscriptions { get; set; }
    /// <summary>每个信箱最多的事件数（默认 100，至少按 1 处理）；满时丢最旧并计数。</summary>
    public int? MaxInboxEvents { get; set; }
    /// <summary>信箱中事件的保留时长（默认 24 小时）；过期的在下次读写该信箱时清理。</summary>
    public TimeSpan? InboxTtl { get; set; }
    /// <summary>每个订阅每分钟（滑动窗口）最多入箱的事件数（默认 60）；0 不限。</summary>
    public int? PerSubscriptionPerMinute { get; set; }

    internal JsonObject ToJson()
    {
        var o = new JsonObject();
        HubOptions.AddCount(o, "maxSubscriptions", MaxSubscriptions, nameof(MaxSubscriptions));
        HubOptions.AddCount(o, "maxInboxEvents", MaxInboxEvents, nameof(MaxInboxEvents));
        HubOptions.AddMs(o, "inboxTtlMs", InboxTtl);
        HubOptions.AddCount(o, "perSubscriptionPerMinute", PerSubscriptionPerMinute, nameof(PerSubscriptionPerMinute));
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
