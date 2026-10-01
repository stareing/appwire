using System.Text.Json;
using System.Text.Json.Nodes;
using AppMcp.Hub.Internal;
using AppMcp.Hub.Native;

namespace AppMcp.Hub;

/// <summary>
/// 嵌入式 app-mcp Hub（Agent 端）：列出并调用各 App 暴露的工具，接收事件，接管审批与配对，
/// 并按 OpenAI / Anthropic / Gemini / MCP 格式导出工具与执行模型返回的工具调用。
/// </summary>
/// <remarks>
/// <para>线程：<see cref="Event"/>、<see cref="ApprovalHandler"/>、<see cref="PairingHandler"/> 在
/// <see cref="HubOptions.Dispatcher"/> 上执行（默认为 <see cref="Start(HubOptions)"/> 时的
/// <see cref="SynchronizationContext.Current"/>）。调度器为 null 时事件直接在 Hub 的分发线程上触发
/// （必须尽快返回），审批 / 配对 handler 在线程池上执行。</para>
/// <para>异步方法（<see cref="CallAsync(CallRequest, CancellationToken)"/> 等）的结果在线程池上完成，await 后回到调用方的上下文。</para>
/// <para>生命周期：<see cref="Dispose"/> 停止 Hub 并释放原生资源（进行中的调用以 CANCELLED 结束，
/// 未完成的审批 / 配对按拒绝处理）。handler 以 GCHandle（强引用）交给原生库，务必显式 Dispose。</para>
/// </remarks>
public sealed class AppMcpHub : IDisposable, IAsyncDisposable
{
    /// <summary>与 crates/hub 的 serde 形式（camelCase）一致的序列化选项。</summary>
    internal static readonly JsonSerializerOptions WireOptions = new(JsonSerializerDefaults.Web);

    private readonly HubSafeHandle _handle;
    private readonly CancellationTokenSource _lifetime = new();
    private Func<ApprovalRequest, CancellationToken, Task<bool>>? _approval;
    private Func<PairingRequest, CancellationToken, Task<bool>>? _pairing;
    private Func<WakeRequest, CancellationToken, Task>? _waker;

    private AppMcpHub(HubSafeHandle handle, SynchronizationContext? dispatcher)
    {
        _handle = handle;
        Dispatcher = dispatcher;
    }

    /// <summary>原生库版本。</summary>
    public static string Version => HubNativeMethods.PtrToString(HubNativeMethods.am_hub_version()) ?? string.Empty;

    /// <summary>事件与 handler 使用的调度器；null 表示分发线程 / 线程池。</summary>
    public SynchronizationContext? Dispatcher { get; }

    /// <summary>参数对象序列化选项（<see cref="CallRequest.Arguments"/>），默认 camelCase。</summary>
    public JsonSerializerOptions SerializerOptions { get; init; } = WireOptions;

    /// <summary>Hub 事件（appConnected、toolsChanged、resourceUpdated、lagged 等）。</summary>
    public event EventHandler<HubEventArgs>? Event;

    /// <summary>用默认配置启动。</summary>
    public static AppMcpHub Start() => Start(new HubOptions());

    /// <summary>创建运行时并启动 Hub。</summary>
    public static AppMcpHub Start(HubOptions options)
    {
        ArgumentNullException.ThrowIfNull(options);
        var dispatcher = options.DispatcherSet ? options.Dispatcher : SynchronizationContext.Current;
        return Start(options.ToConfigJson(), dispatcher);
    }

    /// <summary>用原始 config JSON（字段见 app_mcp_hub.h 的 am_hub_start）启动。</summary>
    public static unsafe AppMcpHub Start(string? configJson, SynchronizationContext? dispatcher)
    {
        nint raw;
        using (var s = new Utf8Strings())
        {
            HubNativeMethods.Check(HubNativeMethods.am_hub_start(s.Add(configJson), out raw));
        }
        var handle = new HubSafeHandle(raw);
        try
        {
            var hub = new AppMcpHub(handle, dispatcher);
            var sink = new HubEventSink(dispatcher);
            sink.Attach(hub);
            var ud = HubCallbacks.Alloc(sink);
            var st = HubNativeMethods.am_hub_set_event_cb(handle, HubCallbacks.EventPtr, ud, HubCallbacks.FreeGCHandlePtr);
            // 失败时 user_data 仍归库所有（库负责调用 free_user_data）。
            HubNativeMethods.Check(st);
            return hub;
        }
        catch
        {
            handle.Dispose();
            throw;
        }
    }

    // -----------------------------------------------------------------------
    // 地址与出口
    // -----------------------------------------------------------------------

    /// <summary>HTTP 服务（/app、/healthz）实际监听的地址（如 "127.0.0.1:52341"，App 端点为 "ws://&lt;地址&gt;/app"）；未开启或已停止时为 null。</summary>
    public string? ListenAddress => HubNativeMethods.TakeString(HubNativeMethods.am_hub_listen_addr(_handle));

    /// <summary>本地 IPC 连接服务的端点（"unix:…" / "pipe:…"，可直接作为 App 端 SDK 的 HostUrl）；未开启或已停止时为 null。</summary>
    public string? IpcEndpoint => HubNativeMethods.TakeString(HubNativeMethods.am_hub_ipc_endpoint(_handle));

    /// <summary>额外启动 Streamable HTTP MCP 出口（路径 /mcp），返回实际监听地址。非回环地址需要 allowRemote。</summary>
    public unsafe string ServeHttp(string address, bool allowRemote = false)
    {
        ArgumentNullException.ThrowIfNull(address);
        using var s = new Utf8Strings();
        HubNativeMethods.Check(HubNativeMethods.am_hub_serve_http(_handle, s.Add(address), allowRemote, out var outAddr));
        return HubNativeMethods.TakeString(outAddr) ?? string.Empty;
    }

    // -----------------------------------------------------------------------
    // 查询
    // -----------------------------------------------------------------------

    /// <summary>AppInfo 数组（含上游，kind = "upstream"）。</summary>
    public JsonElement GetApps()
    {
        HubNativeMethods.Check(HubNativeMethods.am_hub_apps_json(_handle, out var json));
        return TakeJson(json);
    }

    /// <summary>App 列表（类型化，含 <see cref="AppInfo.DormantInstances"/>）。</summary>
    public IReadOnlyList<AppInfo> ListApps() =>
        GetApps().Deserialize<List<AppInfo>>(WireOptions) ?? [];

    /// <summary>工具列表（类型化；休眠实例的工具 <see cref="HubToolInfo.Availability"/> 为 dormant）。</summary>
    public IReadOnlyList<HubToolInfo> ListTools(ToolFilter? filter = null) =>
        GetTools(filter).Deserialize<List<HubToolInfo>>(WireOptions) ?? [];

    /// <summary>HubTool 数组。</summary>
    public unsafe JsonElement GetTools(ToolFilter? filter = null)
    {
        using var s = new Utf8Strings();
        HubNativeMethods.Check(HubNativeMethods.am_hub_tools_json(_handle, s.Add(filter?.ToJson()), out var json));
        return TakeJson(json);
    }

    /// <summary>HubResource 数组（可带 annotations：MCP 内容注解）。</summary>
    public JsonElement GetResources()
    {
        HubNativeMethods.Check(HubNativeMethods.am_hub_resources_json(_handle, out var json));
        return TakeJson(json);
    }

    /// <summary>运行状态 HubStatus 的原始 JSON（与 GET /status 相同，spec/hub-api.md 3.9）。</summary>
    public JsonElement GetStatus()
    {
        HubNativeMethods.Check(HubNativeMethods.am_hub_status_json(_handle, out var json));
        return TakeJson(json);
    }

    /// <summary>运行状态（类型化）：监听、令牌策略、各 App 状态与最近错误、SDK 诊断上报。</summary>
    public HubStatusInfo Status() =>
        GetStatus().Deserialize<HubStatusInfo>(WireOptions)
        ?? throw new HubException(HubStatus.Internal, "状态 JSON 为空");

    /// <summary>AppOverviewInfo；App 未知或没有总览时为 null。</summary>
    public unsafe JsonElement? GetOverview(string appId)
    {
        ArgumentNullException.ThrowIfNull(appId);
        using var s = new Utf8Strings();
        HubNativeMethods.Check(HubNativeMethods.am_hub_overview_json(_handle, s.Add(appId), out var json));
        var v = TakeJson(json);
        return v.ValueKind == JsonValueKind.Null ? null : v;
    }

    // -----------------------------------------------------------------------
    // 调用
    // -----------------------------------------------------------------------

    /// <summary>调用工具。取消 <paramref name="cancellationToken"/> 会取消调用（结果为 CANCELLED 错误）。
    /// 工具失败不抛异常，见 <see cref="CallOutcome.Error"/>。</summary>
    public async Task<CallOutcome> CallAsync(CallRequest request, CancellationToken cancellationToken = default)
    {
        ArgumentNullException.ThrowIfNull(request);
        var (task, callId) = StartCall(request.ToJson(SerializerOptions));
        await using var reg = cancellationToken.Register(() =>
        {
            try { CancelCall(callId); } catch { }
        });
        return new CallOutcome(Parse(await task.ConfigureAwait(false)));
    }

    /// <summary>调用工具（简写）。</summary>
    public Task<CallOutcome> CallAsync(string name, object? arguments = null, CancellationToken cancellationToken = default) =>
        CallAsync(new CallRequest(name, arguments), cancellationToken);

    private unsafe (Task<string>, string) StartCall(string requestJson)
    {
        var tcs = new TaskCompletionSource<string>(TaskCreationOptions.RunContinuationsAsynchronously);
        var ud = HubCallbacks.Alloc(tcs);
        using var s = new Utf8Strings();
        HubStatus st;
        nint callId;
        try
        {
            st = HubNativeMethods.am_hub_call(_handle, s.Add(requestJson), HubCallbacks.ResultPtr, ud, out callId);
        }
        catch
        {
            HubCallbacks.Free(ud);
            throw;
        }
        if (st != HubStatus.Ok)
        {
            var message = HubNativeMethods.LastError();
            HubCallbacks.Free(ud); // 非 OK 时回调不会被调用
            throw new HubException(st, message);
        }
        return (tcs.Task, HubNativeMethods.TakeString(callId) ?? string.Empty);
    }

    /// <summary>取消进行中的调用（含等待审批中的）；未知 callId 忽略。</summary>
    public unsafe void CancelCall(string callId)
    {
        ArgumentNullException.ThrowIfNull(callId);
        using var s = new Utf8Strings();
        HubNativeMethods.Check(HubNativeMethods.am_hub_cancel_call(_handle, s.Add(callId)));
    }

    /// <summary>读取资源（app-mcp://&lt;appId&gt;/&lt;name&gt;）。失败抛出 <see cref="HubCallException"/>。</summary>
    public async Task<ResourceContents> ReadResourceAsync(string uri)
    {
        ArgumentNullException.ThrowIfNull(uri);
        var json = Parse(await StartAsync((ud, s) =>
        {
            unsafe { return HubNativeMethods.am_hub_read_resource(_handle, s.Add(uri), HubCallbacks.ResultPtr, ud); }
        }).ConfigureAwait(false));
        if (json.TryGetProperty("ok", out var ok))
        {
            return new ResourceContents(
                ok.TryGetProperty("uri", out var u) ? u.GetString() ?? uri : uri,
                OptString(ok, "mimeType"),
                OptString(ok, "text"),
                OptString(ok, "blob"));
        }
        var err = json.TryGetProperty("error", out var e) ? CallOutcome.ParseError(e) : new HubError("INTERNAL", "无法解析读取结果", null);
        throw new HubCallException(err);
    }

    /// <summary>订阅资源变化，之后收到 resourceUpdated 事件。</summary>
    public unsafe void Subscribe(string uri)
    {
        ArgumentNullException.ThrowIfNull(uri);
        using var s = new Utf8Strings();
        HubNativeMethods.Check(HubNativeMethods.am_hub_subscribe(_handle, s.Add(uri)));
    }

    public unsafe void Unsubscribe(string uri)
    {
        ArgumentNullException.ThrowIfNull(uri);
        using var s = new Utf8Strings();
        HubNativeMethods.Check(HubNativeMethods.am_hub_unsubscribe(_handle, s.Add(uri)));
    }

    /// <summary>指定某 App 的目标实例（全局默认）；instanceId 为 null 时清除。</summary>
    public unsafe void SelectInstance(string appId, string? instanceId)
    {
        ArgumentNullException.ThrowIfNull(appId);
        using var s = new Utf8Strings();
        HubNativeMethods.Check(HubNativeMethods.am_hub_select_instance(_handle, s.Add(appId), s.Add(instanceId)));
    }

    /// <summary>清除某会话的状态（已附带的总览、apps.select）；null 为默认会话。</summary>
    public unsafe void ResetSession(string? session = null)
    {
        using var s = new Utf8Strings();
        HubNativeMethods.Check(HubNativeMethods.am_hub_reset_session(_handle, s.Add(session)));
    }

    /// <summary>替换策略规则集（spec/hub-api.md 3.13，命中计数清零）；传空规则集清空。</summary>
    /// <exception cref="HubException">规则不合法（<see cref="HubStatus.InvalidConfig"/>）：之前的规则继续生效，
    /// 原因记入 <see cref="HubStatusInfo.Policy"/> 的 LastError。</exception>
    public unsafe void SetPolicy(HubPolicy policy)
    {
        ArgumentNullException.ThrowIfNull(policy);
        using var s = new Utf8Strings();
        HubNativeMethods.Check(HubNativeMethods.am_hub_set_policy(_handle, s.Add(policy.ToJsonString())));
    }

    // -----------------------------------------------------------------------
    // 工具格式导出与分派
    // -----------------------------------------------------------------------

    /// <summary>按格式导出工具定义（名称已编码为 [a-zA-Z0-9_-]{1,64}），可直接交给对应模型 API。</summary>
    public unsafe JsonElement ExportTools(ToolFormat format, ToolFilter? filter = null)
    {
        using var s = new Utf8Strings();
        HubNativeMethods.Check(HubNativeMethods.am_hub_export_tools(_handle, (int)format, s.Add(filter?.ToJson()), out var json));
        return TakeJson(json);
    }

    /// <summary>执行模型返回的一个工具调用，返回该格式的"工具结果"消息（执行失败也以该格式的错误结果返回）。</summary>
    public async Task<JsonElement> DispatchAsync(ToolFormat format, string toolCallJson, string? session = null)
    {
        ArgumentNullException.ThrowIfNull(toolCallJson);
        return Parse(await StartAsync((ud, s) =>
        {
            unsafe
            {
                return HubNativeMethods.am_hub_dispatch(_handle, (int)format, s.Add(toolCallJson), s.Add(session), HubCallbacks.ResultPtr, ud);
            }
        }).ConfigureAwait(false));
    }

    /// <inheritdoc cref="DispatchAsync(ToolFormat, string, string?)"/>
    public Task<JsonElement> DispatchAsync(ToolFormat format, JsonElement toolCall, string? session = null) =>
        DispatchAsync(format, toolCall.GetRawText(), session);

    /// <inheritdoc cref="DispatchAsync(ToolFormat, string, string?)"/>
    public Task<JsonElement> DispatchAsync(ToolFormat format, JsonNode toolCall, string? session = null) =>
        DispatchAsync(format, toolCall.ToJsonString(), session);

    private Task<string> StartAsync(Func<nint, Utf8Strings, HubStatus> start)
    {
        var tcs = new TaskCompletionSource<string>(TaskCreationOptions.RunContinuationsAsynchronously);
        var ud = HubCallbacks.Alloc(tcs);
        using var s = new Utf8Strings();
        HubStatus st;
        try
        {
            st = start(ud, s);
        }
        catch
        {
            HubCallbacks.Free(ud);
            throw;
        }
        if (st != HubStatus.Ok)
        {
            var message = HubNativeMethods.LastError();
            HubCallbacks.Free(ud);
            return Task.FromException<string>(new HubException(st, message));
        }
        return tcs.Task;
    }

    // -----------------------------------------------------------------------
    // 审批与配对
    // -----------------------------------------------------------------------

    /// <summary>
    /// 调用审批（配合 <see cref="HubOptions.RequireApprovalAtOrAbove"/>）：返回 true 放行，false / 异常为拒绝。
    /// 未设置时需要审批的调用以 USER_REJECTED 结束。CancellationToken 在 Hub 释放时取消。
    /// </summary>
    public Func<ApprovalRequest, CancellationToken, Task<bool>>? ApprovalHandler
    {
        get => _approval;
        set
        {
            SetSlot(HubNativeMethods.am_hub_set_approval_cb, HubCallbacks.ApprovalPtr, value is null ? null
                : new DecisionSink<ApprovalRequest>(value, Dispatcher, _lifetime.Token, HubNativeMethods.am_hub_approval_complete));
            _approval = value;
        }
    }

    /// <summary>
    /// App 配对：设置后，无静态清单或 Origin 不在白名单的 App 首次连接时询问；返回 true 允许。
    /// 注意：设置过之后再设为 null，未知 App 一律拒绝。
    /// </summary>
    public Func<PairingRequest, CancellationToken, Task<bool>>? PairingHandler
    {
        get => _pairing;
        set
        {
            SetSlot(HubNativeMethods.am_hub_set_pairing_cb, HubCallbacks.PairingPtr, value is null ? null
                : new DecisionSink<PairingRequest>(value, Dispatcher, _lifetime.Token, HubNativeMethods.am_hub_pairing_complete));
            _pairing = value;
        }
    }

    /// <summary>
    /// 自定义唤醒（spec/hub-api.md 3.5）：替换默认的系统唤醒实现（如 Android 发送显式广播）。
    /// 调用休眠实例的工具（或未运行而清单声明了 wake 的 App）时，Hub 生成一次性令牌并调用此 handler；
    /// 正常返回表示已发出激活，Hub 随后等待 App 回连（<see cref="HubOptions.WakeTimeout"/>）。
    /// 抛出 <see cref="WakeFailedException"/> 以指定错误类别结束调用，其他异常按 LAUNCH_FAILED。
    /// 设为 null 恢复默认实现。在 <see cref="Dispatcher"/> 上执行（null 时在线程池）。
    /// </summary>
    public Func<WakeRequest, CancellationToken, Task>? Waker
    {
        get => _waker;
        set
        {
            SetSlot(HubNativeMethods.am_hub_set_waker_cb, HubCallbacks.WakerPtr, value is null ? null
                : new WakeSink(value, Dispatcher, _lifetime.Token));
            _waker = value;
        }
    }

    /// <summary>设置 / 清除常驻回调。user_data 交给库之后（无论成败）由库负责释放；
    /// 句柄已释放导致未进入原生调用时在此释放。</summary>
    private void SetSlot(Func<HubSafeHandle, nint, nint, nint, HubStatus> set, nint cb, object? target)
    {
        if (target is null)
        {
            HubNativeMethods.Check(set(_handle, 0, 0, 0));
            return;
        }
        var ud = HubCallbacks.Alloc(target);
        HubStatus st;
        try
        {
            st = set(_handle, cb, ud, HubCallbacks.FreeGCHandlePtr);
        }
        catch
        {
            HubCallbacks.Free(ud);
            throw;
        }
        HubNativeMethods.Check(st);
    }

    // -----------------------------------------------------------------------
    // 生命周期
    // -----------------------------------------------------------------------

    internal void RaiseEvent(HubEventArgs args) => Event?.Invoke(this, args);

    /// <summary>停止 Hub（阻塞）：进行中的调用以 CANCELLED 结束，关闭所有 App 连接与上游子进程。幂等。</summary>
    public void Shutdown()
    {
        if (_handle.IsClosed) return;
        HubNativeMethods.am_hub_shutdown(_handle);
    }

    /// <summary>停止并释放 Hub（阻塞）。</summary>
    public void Dispose()
    {
        try { _lifetime.Cancel(); } catch (ObjectDisposedException) { }
        _handle.Dispose();
    }

    /// <summary>在线程池上释放，避免阻塞 UI 线程。</summary>
    public ValueTask DisposeAsync() => new(Task.Run(Dispose));

    // -----------------------------------------------------------------------
    // JSON
    // -----------------------------------------------------------------------

    private static JsonElement TakeJson(nint p) => Parse(HubNativeMethods.TakeString(p) ?? "null");

    internal static JsonElement Parse(string json)
    {
        using var doc = JsonDocument.Parse(json);
        return doc.RootElement.Clone();
    }

    private static string? OptString(JsonElement o, string name) =>
        o.TryGetProperty(name, out var v) && v.ValueKind == JsonValueKind.String ? v.GetString() : null;
}
