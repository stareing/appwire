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

    private AppMcpClient(ClientSafeHandle handle, SynchronizationContext? dispatcher, JsonSerializerOptions json)
    {
        _handle = handle;
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
        var ext = ToNativeOptions(options, policy);
        ext.Lifecycle = (nint)(&lifecycle);
        ext.OnIdleExit = Callbacks.IdleExitPtr; // user_data 与 callbacks 共用（ClientEventSink）
        NativeMethods.Check(NativeMethods.am_client_new_ex(&config, &callbacks, &ext, out var raw));
        var handle = new ClientSafeHandle(raw);
        try
        {
            var client = new AppMcpClient(handle, dispatcher, json);
            sink.Attach(client);
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

    /// <summary>托管选项 → AmClientOptions（不含 Lifecycle 指针与回调）。</summary>
    /// <remarks>@compat C ABI 的 host_absent_retries 0 = 默认 3、负数 = 一直重连；merge_window_ms 0 = 默认 2000、负数 = 不留窗口；
    /// 托管层 0 表示"一直重连 / 不留窗口"，在此转换。</remarks>
    internal static unsafe AmClientOptions ToNativeOptions(AppMcpClientOptions options, LifecycleOptions l)
    {
        if (l.HostAbsentRetries < 0) throw new ArgumentOutOfRangeException(nameof(l.HostAbsentRetries), "HostAbsentRetries 不能为负数（0 = 一直重连）");
        if (!Enum.IsDefined(options.Heartbeat)) throw new ArgumentOutOfRangeException(nameof(options.Heartbeat), "非法的 Heartbeat");
        var merge = ToMillis(l.MergeWindow, nameof(l.MergeWindow));
        return new AmClientOptions
        {
            StructSize = (uint)sizeof(AmClientOptions),
            ConnectTimeoutMs = ToMillis32(options.ConnectTimeout),
            Heartbeat = (int)options.Heartbeat,
            HostAbsentRetries = l.HostAbsentRetries == 0 ? -1 : l.HostAbsentRetries,
            LegacyTimers = l.LegacyTimers ? (byte)1 : (byte)0,
            MergeWindowMs = merge == 0 ? -1 : (long)Math.Min(merge, long.MaxValue),
            SleepOnBackground = l.SleepOnBackground ? (byte)1 : (byte)0,
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

    /// <inheritdoc cref="ToolScope.RegisterResource(string, string, Func{CancellationToken, Task{object?}}, string?, bool)"/>
    public ResourceRegistration RegisterResource(
        string name,
        string description,
        Func<CancellationToken, Task<object?>> reader,
        string? mimeType = null,
        bool realtime = false) => _root.RegisterResource(name, description, reader, mimeType, realtime);

    /// <inheritdoc cref="ToolScope.RegisterResource{T}"/>
    public ResourceRegistration RegisterResource<T>(
        string name,
        string description,
        Func<CancellationToken, Task<T>> reader,
        string? mimeType = null,
        bool realtime = false) => _root.RegisterResource(name, description, reader, mimeType, realtime);

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
