using Microsoft.UI.Dispatching;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using AppMcp.UiFallback;

namespace AppMcp.WinUI.UiFallback;

/// <summary><see cref="WinUIUiFallback.Enable(AppMcpClient, Window, WinUIUiFallbackOptions?)"/> 的选项。</summary>
public sealed record WinUIUiFallbackOptions
{
    /// <summary>工具名前缀，默认 <c>ui</c>（<c>ui.outline</c> 等）。</summary>
    public string Prefix { get; init; } = "ui";
    /// <summary><c>outline</c> 缺省列出的控件数，默认 60。</summary>
    public int MaxItems { get; init; } = 60;
    /// <summary>操作后在让出调度器之外再等的时长（再取变化摘要），默认 50 ms。</summary>
    public TimeSpan SettleDelay { get; init; } = TimeSpan.FromMilliseconds(50);
}

/// <summary>
/// WinUI 3 进程内控件兜底（spec/ui-fallback.md，第 4c 项 H）：开发者显式开启后注册
/// <c>ui.outline</c> / <c>click</c> / <c>fill</c> / <c>press</c> / <c>scroll</c> / <c>read</c>，经 AutomationPeer
/// （Invoke / Toggle / Value / RangeValue / Scroll 等模式）直接操作控件，不截图、不按坐标。
/// </summary>
/// <remarks>
/// <list type="bullet">
/// <item>WinUI 3 不能枚举应用的窗口：在 <see cref="Enable(AppMcpClient, Window, WinUIUiFallbackOptions?)"/> 传入主窗口，
/// 其他窗口用 <see cref="AddWindow"/> 加入（关闭时自动移除）；</item>
/// <item>工具以 <see cref="ToolSurface.View"/> 注册，只在有可见（未最小化）的窗口时启用；</item>
/// <item>每个动作在 UI 线程上按引用重新取控件，核对仍在界面上、可见（不在屏外、未被 ContentDialog / 浮出层遮挡）、启用；</item>
/// <item>PasswordBox 的值只显示 <c>••••</c>，拒绝填写与按键；</item>
/// <item>经 <see cref="WinUIViewTools.Bind(FrameworkElement, Window?, ToolRegistration[])"/> 绑定了工具的控件在大纲中标出 <c>[已声明：…]</c>。</item>
/// </list>
/// 必须在 UI 线程上调用；handler 不在 UI 线程时自动切到创建时的 <see cref="DispatcherQueue"/>。
/// </remarks>
/// <example>
/// <code>
/// #if DEBUG
/// _fallback = WinUIUiFallback.Enable(client, mainWindow);   // _fallback.Dispose() 注销全部兜底工具
/// #endif
/// </code>
/// </example>
public sealed class WinUIUiFallback : IDisposable
{
    private readonly ToolScope _scope;
    private readonly DispatcherQueue _dispatcher;
    private readonly List<ToolRegistration> _tools = [];
    private readonly List<Window> _windows = [];
    private bool _enabled;
    private bool _disposed;

    internal WinUIUiInspector Inspector { get; }

    private WinUIUiFallback(ToolScope scope, WinUIUiFallbackOptions options)
    {
        _scope = scope;
        _dispatcher = DispatcherQueue.GetForCurrentThread() ?? throw new InvalidOperationException("必须在 UI 线程上开启兜底工具");
        Inspector = new WinUIUiInspector(options.Prefix, Math.Max(1, options.MaxItems), options.SettleDelay, () => _windows, WinUIViewTools.DeclaredTools);
    }

    /// <summary>在客户端根作用域下建子作用域 <c>&lt;prefix&gt;-fallback</c> 并注册兜底工具。</summary>
    public static WinUIUiFallback Enable(AppMcpClient client, Window window, WinUIUiFallbackOptions? options = null)
    {
        ArgumentNullException.ThrowIfNull(client);
        options ??= new WinUIUiFallbackOptions();
        return Start(client.CreateScope($"{options.Prefix}-fallback"), window, options);
    }

    /// <summary>在 <paramref name="parent"/> 下建子作用域 <c>&lt;prefix&gt;-fallback</c> 并注册兜底工具。</summary>
    public static WinUIUiFallback Enable(ToolScope parent, Window window, WinUIUiFallbackOptions? options = null)
    {
        ArgumentNullException.ThrowIfNull(parent);
        options ??= new WinUIUiFallbackOptions();
        return Start(parent.CreateScope($"{options.Prefix}-fallback"), window, options);
    }

    private static WinUIUiFallback Start(ToolScope scope, Window window, WinUIUiFallbackOptions options)
    {
        ArgumentNullException.ThrowIfNull(window);
        WinUIUiFallback? fallback = null;
        try
        {
            fallback = new WinUIUiFallback(scope, options);
            fallback._tools.AddRange(UiFallbackTools.Register(scope, fallback.Inspector, fallback.OnUi));
            fallback.AddWindow(window);
            return fallback;
        }
        catch
        {
            if (fallback is not null) fallback.Dispose();
            else scope.Dispose();
            throw;
        }
    }

    /// <summary>兜底工具当前是否启用（有可见窗口）。</summary>
    public bool Enabled => !_disposed && _enabled;

    /// <summary>纳入兜底范围的另一个窗口（关闭时自动移除）。必须在 UI 线程上调用。</summary>
    public void AddWindow(Window window)
    {
        ArgumentNullException.ThrowIfNull(window);
        if (_disposed || _windows.Contains(window)) return;
        _windows.Add(window);
        window.VisibilityChanged += OnVisibilityChanged;
        window.Closed += OnClosed;
        window.AppWindow.Changed += OnAppWindowChanged;
        Recompute();
    }

    private void Detach(Window window)
    {
        window.VisibilityChanged -= OnVisibilityChanged;
        window.Closed -= OnClosed;
        window.AppWindow.Changed -= OnAppWindowChanged;
    }

    private void OnVisibilityChanged(object sender, WindowVisibilityChangedEventArgs e) => Recompute();

    private void OnAppWindowChanged(AppWindow sender, AppWindowChangedEventArgs e)
    {
        if (e.DidPresenterChange || e.DidVisibilityChange) Recompute();
    }

    private void OnClosed(object sender, WindowEventArgs e)
    {
        if (sender is Window w)
        {
            Detach(w);
            _windows.Remove(w);
        }
        Recompute();
    }

    /// <summary>窗口可被操作：可见且未最小化。</summary>
    internal static bool IsUsable(Window w) =>
        w.Visible && w.AppWindow is { IsVisible: true } app && app.Presenter is not OverlappedPresenter { State: OverlappedPresenterState.Minimized };

    private void Recompute()
    {
        if (_disposed) return;
        var visible = _windows.Any(IsUsable);
        if (visible == _enabled) return;
        _enabled = visible;
        foreach (var t in _tools) t.SetEnabled(visible);
        if (!visible) Inspector.Clear();
    }

    /// <summary>切到 UI 线程并核对启用后执行。</summary>
    private Task<object?> OnUi(Func<Task<object?>> run)
    {
        if (_dispatcher.HasThreadAccess) return Guarded(run);
        var done = new TaskCompletionSource<object?>(TaskCreationOptions.RunContinuationsAsynchronously);
        var queued = _dispatcher.TryEnqueue(async () =>
        {
            try { done.TrySetResult(await Guarded(run)); }
            catch (Exception e) { done.TrySetException(e); }
        });
        if (!queued) done.TrySetException(new ToolCallException(ToolErrorKind.ToolDisabled, "UI 线程已退出，兜底工具不可用"));
        return done.Task;
    }

    private Task<object?> Guarded(Func<Task<object?>> run)
    {
        if (!Enabled) throw new ToolCallException(ToolErrorKind.ToolDisabled, "应用当前没有可见窗口，兜底工具不可用");
        return run();
    }

    /// <summary>注销全部兜底工具（幂等）。</summary>
    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;
        foreach (var w in _windows) Detach(w);
        _windows.Clear();
        Inspector.Clear();
        _tools.Clear();
        _scope.Dispose();
    }
}
