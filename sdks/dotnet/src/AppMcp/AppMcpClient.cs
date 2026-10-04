using System.Text.Json;
using AppMcp.Internal;
using AppMcp.Native;

namespace AppMcp;

/// <summary>
/// app-mcp 客户端：把 App 内的业务动作以 MCP 工具暴露给模型。
/// </summary>
/// <remarks>
/// <para>线程：handler 与事件在 <see cref="AppMcpClientOptions.Dispatcher"/> 上执行
/// （默认为 <see cref="Create"/> 时的 <see cref="SynchronizationContext.Current"/>）。
/// 在 WPF / WinUI 的 UI 线程上创建客户端，handler 即自动在 UI 线程执行。</para>
/// <para>生命周期：<see cref="Dispose"/> 停止客户端并阻塞到后台线程结束；不要在库的分发线程上调用。
/// UI 线程上建议用 <see cref="DisposeAsync"/>。</para>
/// </remarks>
public sealed class AppMcpClient : IDisposable, IAsyncDisposable
{
    private readonly ClientSafeHandle _handle;
    private readonly ToolScope _root;
    private readonly BusyState _busy;

    private AppMcpClient(ClientSafeHandle handle, SynchronizationContext? dispatcher, JsonSerializerOptions json)
    {
        _handle = handle;
        _busy = new BusyState(busy => NativeMethods.Check(NativeMethods.am_client_set_busy(_handle, busy)));
        Dispatcher = dispatcher;
        SerializerOptions = json;
        NativeMethods.Check(NativeMethods.am_client_root_scope(_handle, out var root));
        _root = new ToolScope(new ScopeSafeHandle(root), this, isRoot: true);
    }

    /// <summary>原生库版本。</summary>
    public static string Version => NativeMethods.PtrToString(NativeMethods.am_version()) ?? string.Empty;

    /// <summary>handler 与事件使用的调度器；null 表示线程池。</summary>
    public SynchronizationContext? Dispatcher { get; }

    public JsonSerializerOptions SerializerOptions { get; }

    /// <summary>状态变化（在 <see cref="Dispatcher"/> 上触发）。</summary>
    public event EventHandler<ClientStateChangedEventArgs>? StateChanged;

    /// <summary>配对成功并获得新 token，App 应持久化。</summary>
    public event EventHandler<PairedEventArgs>? Paired;

    public event EventHandler<LogEventArgs>? Log;

    /// <summary>
    /// 已进入休眠，且 <see cref="LifecycleOptions.Residency"/> 允许退出进程（在 <see cref="Dispatcher"/> 上触发）。
    /// App 自行决定是否退出，例如 WPF：<c>Application.Current.Shutdown()</c>。
    /// </summary>
    public event EventHandler? IdleExit;

    /// <summary>创建客户端并启动后台线程（不连接，调用 <see cref="Start"/> 后开始连接）。</summary>
    public static unsafe AppMcpClient Create(AppMcpClientOptions options)
    {
        ArgumentNullException.ThrowIfNull(options);
        if (options.MaxConcurrentCalls < 0) throw new ArgumentOutOfRangeException(nameof(options), "MaxConcurrentCalls 不能为负数");
        var dispatcher = options.DispatcherSet ? options.Dispatcher : SynchronizationContext.Current;
        var json = ToolSchema.Prepare(options.SerializerOptions);

        var sink = new ClientEventSink(dispatcher);
        using var strings = new Utf8Strings();
        var config = new AmClientConfig
        {
            AppId = strings.Add(options.AppId),
            AppName = strings.Add(options.AppName),
            InstanceId = strings.Add(options.InstanceId),
            HostUrl = strings.Add(options.HostUrl),
            AppVersion = strings.Add(options.AppVersion),
            InstanceTitle = strings.Add(options.InstanceTitle),
            Token = strings.Add(options.Token),
            LaunchToken = strings.Add(options.LaunchToken),
            ClientKind = (int)options.ClientKind,
            MaxConcurrentCalls = (uint)options.MaxConcurrentCalls,
            OverviewSummary = strings.Add(options.Overview?.Summary),
            OverviewBody = strings.Add(options.Overview?.Body),
            OverviewLocale = strings.Add(options.Overview?.Locale),
        };
        var callbacks = new AmClientCallbacks
        {
            OnState = Callbacks.StatePtr,
            OnPaired = Callbacks.PairedPtr,
            OnLog = Callbacks.LogPtr,
            UserData = Callbacks.Alloc(sink), // 由库在释放客户端（或创建失败）时释放
            FreeUserData = Callbacks.FreeGCHandlePtr,
        };
        var policy = options.Lifecycle ?? new LifecycleOptions();
        var lifecycle = ToNative(policy, strings);
        var ext = ToNativeOptions(options, policy, strings);
        ext.Lifecycle = (nint)(&lifecycle);
        ext.OnIdleExit = Callbacks.IdleExitPtr; // user_data 与 callbacks 共用（ClientEventSink）
        NativeMethods.Check(NativeMethods.am_client_new_ex(&config, &callbacks, &ext, out var raw));
        var handle = new ClientSafeHandle(raw);
        try
        {
            var client = new AppMcpClient(handle, dispatcher, json);
            sink.Attach(client);
            if (options.NavigateInBackground is { } navigateInBackground) client.SetNavigateInBackground(navigateInBackground);
            // @why AmClientOptions 不含 busy 策略（app_mcp.h v19 结构体不变），创建后立即设置；非法值由原生库拒绝。
            client.SetBusyPolicy(options.BusyPolicy);
            return client;
        }
        catch
        {
            handle.Dispose();
            throw;
        }
    }

    /// <summary>托管的生命周期策略 → C 结构体（字符串分配在 <paramref name="strings"/> 中）。</summary>
    internal static AmLifecycle ToNative(LifecycleOptions l, Utf8Strings strings) => new()
    {
        Mode = (int)l.Mode,
        IdleTimeoutMs = ToMillis(l.IdleTimeout, nameof(l.IdleTimeout)),
        HiddenIdleTimeoutMs = ToMillis(l.HiddenIdleTimeout, nameof(l.HiddenIdleTimeout)),
        GraceMs = ToMillis(l.Grace, nameof(l.Grace)),
        Residency = (int)l.Residency,
        WakeKind = l.Wake is null ? -1 : (int)l.Wake.Kind,
        WakeTarget = strings.Add(l.Wake?.Target),
        WakeBackground = l.Wake?.Background == true ? (byte)1 : (byte)0,
    };

    /// <summary>托管选项 → AmClientOptions（不含 Lifecycle 指针与回调；NameInstance 字符串分配在 <paramref name="strings"/> 中）。</summary>
    /// <remarks>@compat C ABI 的 host_absent_retries 0 = 默认 3、负数 = 一直重连；merge_window_ms 0 = 默认 2000、负数 = 不留窗口；
    /// call_dedup_* 0 = 默认、负数 = 关闭；max_queued_calls 0 = 默认 64、负数 = 不限；
    /// 托管层 0 表示"一直重连 / 不留窗口 / 关闭去重 / 不限排队"，在此转换。</remarks>
    internal static unsafe AmClientOptions ToNativeOptions(AppMcpClientOptions options, LifecycleOptions l, Utf8Strings strings)
    {
        if (l.HostAbsentRetries < 0) throw new ArgumentOutOfRangeException(nameof(l.HostAbsentRetries), "HostAbsentRetries 不能为负数（0 = 一直重连）");
        if (!Enum.IsDefined(options.Heartbeat)) throw new ArgumentOutOfRangeException(nameof(options.Heartbeat), "非法的 Heartbeat");
        var merge = ToMillis(l.MergeWindow, nameof(l.MergeWindow));
        var dedup = options.CallDedup ?? new CallDedupOptions();
        if (dedup.MaxEntries < 0) throw new ArgumentOutOfRangeException(nameof(dedup.MaxEntries), "CallDedup.MaxEntries 不能为负数（0 = 关闭）");
        var dedupTtl = ToMillis(dedup.Ttl, nameof(dedup.Ttl));
        if (options.MaxQueuedCalls < 0) throw new ArgumentOutOfRangeException(nameof(options.MaxQueuedCalls), "MaxQueuedCalls 不能为负数（0 = 不限）");
        return new AmClientOptions
        {
            StructSize = (uint)sizeof(AmClientOptions),
            ConnectTimeoutMs = ToMillis32(options.ConnectTimeout),
            Heartbeat = (int)options.Heartbeat,
            HostAbsentRetries = l.HostAbsentRetries == 0 ? -1 : l.HostAbsentRetries,
            LegacyTimers = l.LegacyTimers ? (byte)1 : (byte)0,
            MergeWindowMs = merge == 0 ? -1 : (long)Math.Min(merge, long.MaxValue),
            SleepOnBackground = l.SleepOnBackground ? (byte)1 : (byte)0,
            CallDedupTtlMs = dedupTtl == 0 ? -1 : (long)Math.Min(dedupTtl, long.MaxValue),
            CallDedupMaxEntries = dedup.MaxEntries == 0 ? -1 : dedup.MaxEntries,
            RegisterName = options.RegisterName ? (byte)1 : (byte)0,
            NameInstance = strings.Add(options.NameInstance),
            MaxQueuedCalls = options.MaxQueuedCalls == 0 ? -1 : options.MaxQueuedCalls,
        };
    }

    private static ulong ToMillis(TimeSpan t, string name) =>
        t < TimeSpan.Zero ? throw new ArgumentOutOfRangeException(name, "时间不能为负数") : (ulong)t.TotalMilliseconds;

    /// <summary>null → 0（C 侧默认值）。</summary>
    internal static uint ToMillis32(TimeSpan? t)
    {
        if (t is not { } v) return 0;
        if (v <= TimeSpan.Zero) throw new ArgumentOutOfRangeException(nameof(t), "ConnectTimeout 必须大于 0");
        return (uint)Math.Min(uint.MaxValue, Math.Max(1, v.TotalMilliseconds));
    }

    /// <summary>开始连接 Host。重复调用无效果。</summary>
    public void Start() => NativeMethods.Check(NativeMethods.am_client_start(_handle));

    /// <summary>停止：取消所有调用、断开连接、不再重连。</summary>
    public void Stop() => NativeMethods.Check(NativeMethods.am_client_stop(_handle));

    /// <summary>
    /// 设置导航回调（Host 的 <c>app/navigate</c>，spec/protocol.md 3.4）；null 清除（之后的导航请求以 NAVIGATION_FAILED 回复）。
    /// </summary>
    /// <remarks>
    /// <para>handler 在 <see cref="Dispatcher"/> 上执行（WPF / WinUI 即 UI 线程）：切换到目标页面，最好等新页面的工具注册之后再返回。
    /// 正常返回 = 导航完成；抛出 <see cref="NavigationDeniedException"/> = 拒绝（如用户正在输入）；
    /// 抛出 <see cref="UserActionRequiredException"/> = USER_ACTION_REQUIRED（如后台时发通知，reason "foreground" + uri）；其他异常 = 失败。</para>
    /// <para>能力在握手时声明：建议在 <see cref="Start"/> 之前设置，连接后才设置的在下次连接时生效。</para>
    /// </remarks>
    public void SetNavigationHandler(Func<NavigationRequest, Task>? handler)
    {
        if (handler is null)
        {
            NativeMethods.Check(NativeMethods.am_client_set_navigation_handler(_handle, 0, 0, 0));
            return;
        }
        var invoker = new NavigationInvoker(handler, Dispatcher);
        // user_data 的所有权交给库：替换 / 清除 / 释放客户端（及设置失败）时库调用 FreeGCHandle。
        NativeMethods.Check(NativeMethods.am_client_set_navigation_handler(
            _handle, Callbacks.NavigatePtr, Callbacks.Alloc(invoker), Callbacks.FreeGCHandlePtr));
    }

    /// <summary>同步版本的 <see cref="SetNavigationHandler(Func{NavigationRequest, Task}?)"/>（如 WPF 的 <c>Frame.Navigate</c>）。</summary>
    public void SetNavigationHandler(Action<NavigationRequest>? handler) =>
        SetNavigationHandler(handler is null ? null : request =>
        {
            handler(request);
            return Task.CompletedTask;
        });

    /// <summary>App 在后台时是否仍把导航交给导航回调（见 <see cref="AppMcpClientOptions.NavigateInBackground"/>）；对之后到达的请求生效。</summary>
    public void SetNavigateInBackground(bool enabled) =>
        NativeMethods.Check(NativeMethods.am_client_set_navigate_in_background(_handle, enabled));

    // ---- 用户正在操作（spec/protocol.md 5.3） ------------------------------

    /// <summary>
    /// 显式开关：声明用户正在 / 不再在 App 内操作。期间写调用（生效注解不是 <c>readOnlyHint: true</c> 的工具）按
    /// <see cref="AppMcpClientOptions.BusyPolicy"/> 拒绝或排队；只读调用与已开始的调用不受影响。何时算"正在操作"由 App 决定，随时生效。
    /// </summary>
    /// <remarks>有效 busy = 本开关 ∨ 未结束的 <see cref="Busy"/> 作用域数 &gt; 0：<c>SetBusy(false)</c> 不结束进行中的作用域。</remarks>
    public void SetBusy(bool busy)
    {
        ObjectDisposedException.ThrowIf(_handle.IsClosed, this);
        _busy.Set(busy);
    }

    /// <summary>有效 busy（显式开关 ∨ 未结束的作用域数 &gt; 0）。</summary>
    public bool IsBusy
    {
        get
        {
            ObjectDisposedException.ThrowIf(_handle.IsClosed, this);
            return _busy.Effective;
        }
    }

    /// <summary>修改用户正在操作期间写调用的处理方式；随即对排队中的调用生效（改为 Reject 时排队的写调用被拒绝）。</summary>
    /// <exception cref="AppMcpException">非法值（<see cref="AppMcpStatus.InvalidArgument"/>）。</exception>
    public void SetBusyPolicy(BusyPolicy policy) =>
        NativeMethods.Check(NativeMethods.am_client_set_busy_policy(_handle, (int)policy));

    /// <summary>开始一个用户正在操作的作用域：引用计数 +1，返回的对象 Dispose 时 -1（重复 Dispose 无效果）。可嵌套、可跨线程 Dispose。</summary>
    /// <remarks>作用域结束不清 <see cref="SetBusy"/> 的显式开关；全部作用域结束且开关为 false 时才解除。</remarks>
    public IDisposable Busy()
    {
        ObjectDisposedException.ThrowIf(_handle.IsClosed, this);
        _busy.Enter();
        return new BusyRelease(_busy);
    }

    // ---- 事件（spec/protocol.md 3.5） ---------------------------------------

    /// <summary>
    /// 声明本实例可发出的事件（同名替换）。已连接时随即同步给 Host，否则在下次握手成功后同步；不触发连接。
    /// SDK 不读清单：要发出的事件都需在运行时声明。
    /// </summary>
    /// <param name="name">事件名，规则同工具名，如 <c>order.shipped</c>。</param>
    /// <param name="description">面向模型：事件何时发生、载荷含义。</param>
    /// <param name="payloadSchemaJson">载荷的 JSON Schema（对象文本，描述用，Hub 不校验）；null = 不声明。</param>
    /// <exception cref="AppMcpException">名称不合法（<see cref="AppMcpStatus.InvalidName"/>）、schema 不是 JSON 对象
    /// （<see cref="AppMcpStatus.InvalidSchema"/>）、客户端已停止（<see cref="AppMcpStatus.Stopped"/>）。</exception>
    public unsafe void DeclareEvent(string name, string description, string? payloadSchemaJson = null)
    {
        ArgumentNullException.ThrowIfNull(name);
        ArgumentNullException.ThrowIfNull(description);
        using var strings = new Utf8Strings();
        NativeMethods.Check(NativeMethods.am_client_declare_event(
            _handle, strings.AddPtr(name), strings.AddPtr(description), strings.AddPtr(payloadSchemaJson)));
    }

    /// <summary>撤销事件声明；返回是否撤销了已有声明（未声明过为 false）。</summary>
    public unsafe bool RemoveEvent(string name)
    {
        ArgumentNullException.ThrowIfNull(name);
        using var strings = new Utf8Strings();
        byte removed = 0;
        NativeMethods.Check(NativeMethods.am_client_remove_event(_handle, strings.AddPtr(name), &removed));
        return removed != 0;
    }

    /// <summary>
    /// 发出已声明的事件。<paramref name="payload"/> 按 <see cref="SerializerOptions"/> 序列化，须为 JSON 对象（null = 无载荷）。
    /// 已连接时发送并返回 true；未连接（休眠、断线、重连中、握手中）时丢弃并返回 false——不缓存、不为此连接或唤醒 Host，
    /// 也不推迟空闲休眠。需要可靠送达的状态变化请改用资源。
    /// </summary>
    /// <exception cref="AppMcpException">未声明 / 名称不合法（<see cref="AppMcpStatus.InvalidName"/>）、载荷不是对象或超过 8 KiB
    /// （<see cref="AppMcpStatus.InvalidJson"/>）、客户端已停止（<see cref="AppMcpStatus.Stopped"/>）。</exception>
    public bool EmitEvent(string name, object? payload = null) =>
        EmitEventJson(name, payload is null ? null : JsonSerializer.Serialize(payload, payload.GetType(), SerializerOptions));

    /// <summary>同 <see cref="EmitEvent"/>，载荷为 JSON 对象文本（null = 无载荷）。</summary>
    public unsafe bool EmitEventJson(string name, string? payloadJson)
    {
        ArgumentNullException.ThrowIfNull(name);
        using var strings = new Utf8Strings();
        byte sent = 0;
        NativeMethods.Check(NativeMethods.am_client_emit_event(_handle, strings.AddPtr(name), strings.AddPtr(payloadJson), &sent));
        return sent != 0;
    }

    public void SetVisibility(AppVisibility visibility, bool focused) =>
        NativeMethods.Check(NativeMethods.am_client_set_visibility(_handle, (int)visibility, focused));

    public ClientState State
    {
        get
        {
            NativeMethods.Check(NativeMethods.am_client_state(_handle, out var status, out var retry, out var reason));
            var st = (ClientStatus)status;
            var reasonText = NativeMethods.TakeString(reason);
            return new ClientState(st, st == ClientStatus.Backoff ? TimeSpan.FromMilliseconds(retry) : null, reasonText)
            {
                Code = QueryStateCode(),
            };
        }
    }

    /// <summary>当前状态的错误码（am_client_state_code）。</summary>
    internal string? QueryStateCode()
    {
        NativeMethods.Check(NativeMethods.am_client_state_code(_handle, out var code));
        return NativeMethods.TakeString(code);
    }

    public string InstanceId => NativeMethods.TakeString(NativeMethods.am_client_instance_id(_handle)) ?? string.Empty;

    /// <summary>当前 token（配置带入的或配对后获得的）。</summary>
    public string? Token => NativeMethods.TakeString(NativeMethods.am_client_token(_handle));

    /// <summary>Host 为当前连接分配的连接 ID（spec/protocol.md 10.3），与 Host 日志中的 <c>cid</c> 对应；未连接时为 null。</summary>
    public string? ConnectionId
    {
        get
        {
            NativeMethods.Check(NativeMethods.am_client_connection_id(_handle, out var id));
            return NativeMethods.TakeString(id);
        }
    }

    // ---- 生命周期（spec/lifecycle.md 第 8 节） ------------------------------

    /// <summary>
    /// 处理操作系统激活参数 / URL（命令行参数、协议激活 URI、AUMID 激活参数），识别
    /// <c>app-mcp-wake:&lt;token&gt;</c>、<c>&lt;scheme&gt;:app-mcp/wake?token=</c> 等。不是本 SDK 的唤醒返回 false。
    /// 可以在 <see cref="Start"/> 之前调用（冷启动唤醒）。
    /// </summary>
    public unsafe bool HandleWake(string args)
    {
        ArgumentNullException.ThrowIfNull(args);
        using var strings = new Utf8Strings();
        return NativeMethods.am_client_handle_wake(_handle, strings.AddPtr(args));
    }

    /// <summary>逐个检查命令行参数（如 <c>Environment.GetCommandLineArgs()</c> 或 <c>StartupEventArgs.Args</c>），任一识别即返回 true。</summary>
    public bool HandleWake(IEnumerable<string> args)
    {
        ArgumentNullException.ThrowIfNull(args);
        foreach (var a in args)
        {
            if (a is not null && HandleWake(a)) return true;
        }
        return false;
    }

    /// <summary>从激活参数中提取唤醒令牌（不需要客户端）；不是唤醒参数时返回 null。</summary>
    public static unsafe string? ParseWakeToken(string args)
    {
        ArgumentNullException.ThrowIfNull(args);
        using var strings = new Utf8Strings();
        return NativeMethods.TakeString(NativeMethods.am_parse_wake_token(strings.AddPtr(args)));
    }

    /// <summary>判断一组命令行参数中是否含唤醒参数。</summary>
    public static bool IsWakeArgs(IEnumerable<string> args) => args.Any(a => a is not null && ParseWakeToken(a) is not null);

    /// <summary>App 主动回连（如用户打开了相关界面）。返回是否因此发起了回连。</summary>
    public unsafe bool Wake(WakeReason reason = WakeReason.App)
    {
        byte started = 0;
        NativeMethods.Check(NativeMethods.am_client_wake_with_reason(_handle, (int)reason, &started));
        return started != 0;
    }

    /// <summary><see cref="LifecycleMode.OnDemand"/> 下主动连接；尚未 <see cref="Start"/> 时等同于 Start。</summary>
    public unsafe bool ConnectNow()
    {
        byte started = 0;
        NativeMethods.Check(NativeMethods.am_client_connect_now(_handle, &started));
        return started != 0;
    }

    /// <summary>App 主动请求休眠（默认原因 app，不受空闲条件与持有影响）。返回是否有效果。</summary>
    public unsafe bool Sleep(SleepReason reason = SleepReason.App)
    {
        byte changed = 0;
        NativeMethods.Check(NativeMethods.am_client_sleep_with_reason(_handle, (int)reason, &changed));
        return changed != 0;
    }

    /// <summary>临时阻止自动休眠，直到返回的对象被 Dispose。</summary>
    public IDisposable Hold()
    {
        NativeMethods.Check(NativeMethods.am_client_hold(_handle, out var raw));
        return new HoldRelease(new HoldSafeHandle(raw));
    }

    /// <summary>当前工具与资源定义的摘要（spec/lifecycle.md 第 6 节）。</summary>
    public string ToolsHash => NativeMethods.TakeString(NativeMethods.am_client_tools_hash(_handle))
        ?? throw new AppMcpException(AppMcpStatus.Stopped, NativeMethods.LastError());

    /// <summary>创建作用域（Dispose 时注销其下全部工具与资源）。</summary>
    public ToolScope CreateScope(string name) => _root.CreateScope(name);

    /// <inheritdoc cref="ToolScope.RegisterTool(string, string, Func{JsonElement, ToolContext, Task{object?}}, ToolOptions?)"/>
    public ToolRegistration RegisterTool(
        string name,
        string description,
        Func<JsonElement, ToolContext, Task<object?>> handler,
        ToolOptions? options = null) => _root.RegisterTool(name, description, handler, options);

    /// <inheritdoc cref="ToolScope.RegisterTool{TInput, TOutput}"/>
    public ToolRegistration RegisterTool<TInput, TOutput>(
        string name,
        string description,
        Func<TInput, ToolContext, Task<TOutput>> handler,
        ToolOptions? options = null) => _root.RegisterTool(name, description, handler, options);

    /// <inheritdoc cref="ToolScope.RegisterResource(string, string, Func{CancellationToken, Task{object?}}, string?, bool, ContentAnnotations?, CachePolicy?)"/>
    public ResourceRegistration RegisterResource(
        string name,
        string description,
        Func<CancellationToken, Task<object?>> reader,
        string? mimeType = null,
        bool realtime = false,
        ContentAnnotations? annotations = null,
        CachePolicy? cache = null) => _root.RegisterResource(name, description, reader, mimeType, realtime, annotations, cache);

    /// <inheritdoc cref="ToolScope.RegisterResource{T}"/>
    public ResourceRegistration RegisterResource<T>(
        string name,
        string description,
        Func<CancellationToken, Task<T>> reader,
        string? mimeType = null,
        bool realtime = false,
        ContentAnnotations? annotations = null,
        CachePolicy? cache = null) => _root.RegisterResource(name, description, reader, mimeType, realtime, annotations, cache);

    /// <summary>注销全部工具与资源（客户端继续运行）。</summary>
    public void UnregisterAll() => _root.Unregister();

    internal void RaiseStateChanged(ClientState state) => StateChanged?.Invoke(this, new ClientStateChangedEventArgs(state));
    internal void RaisePaired(string token) => Paired?.Invoke(this, new PairedEventArgs(token));
    internal void RaiseLog(LogLevel level, string message) => Log?.Invoke(this, new LogEventArgs(level, message));
    internal void RaiseIdleExit() => IdleExit?.Invoke(this, EventArgs.Empty);

    /// <summary>停止并释放客户端（阻塞到后台线程结束）。</summary>
    public void Dispose()
    {
        _root.Dispose();
        _handle.Dispose();
    }

    /// <summary>在线程池上释放，避免阻塞 UI 线程。</summary>
    public ValueTask DisposeAsync() => new(Task.Run(Dispose));
}
