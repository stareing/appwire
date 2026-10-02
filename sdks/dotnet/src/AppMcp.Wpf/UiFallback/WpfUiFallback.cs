using System.Text.Json;
using System.Windows;
using AppMcp.UiFallback;

namespace AppMcp.Wpf.UiFallback;

/// <summary><see cref="WpfUiFallback.Enable(AppMcpClient, WpfUiFallbackOptions?)"/> 的选项。</summary>
public sealed record WpfUiFallbackOptions
{
    /// <summary>工具名前缀，默认 <c>ui</c>（<c>ui.outline</c> 等）。</summary>
    public string Prefix { get; init; } = "ui";
    /// <summary><c>outline</c> 缺省列出的控件数，默认 60。</summary>
    public int MaxItems { get; init; } = 60;
    /// <summary>操作后在调度器空闲之外再等的时长（再取变化摘要），默认 50 ms。</summary>
    public TimeSpan SettleDelay { get; init; } = TimeSpan.FromMilliseconds(50);
}

/// <summary>
/// WPF 进程内控件兜底（spec/ui-fallback.md，第 4c 项 H）：开发者显式开启后注册
/// <c>ui.outline</c> / <c>click</c> / <c>fill</c> / <c>press</c> / <c>scroll</c> / <c>read</c>，经 AutomationPeer 直接操作控件，
/// 不截图、不按坐标。
/// </summary>
/// <remarks>
/// <list type="bullet">
/// <item>工具以 <see cref="ToolSurface.View"/> 注册，只在应用有可见（未最小化）窗口时启用；</item>
/// <item>每个动作在 UI 线程上按引用重新取控件，核对仍在界面上、可见（所在窗口未被模态对话框禁用、不在屏外）、启用；</item>
/// <item>PasswordBox 的值只显示 <c>••••</c>，拒绝填写与按键；</item>
/// <item>经 <see cref="WpfViewTools.Bind(FrameworkElement, ToolRegistration[])"/> 绑定了工具的控件在大纲中标出 <c>[已声明：…]</c>。</item>
/// </list>
/// 必须在 UI 线程上调用 <see cref="Enable(AppMcpClient, WpfUiFallbackOptions?)"/>；handler 不在 UI 线程时自动切到
/// <c>Application.Current.Dispatcher</c>。
/// </remarks>
/// <example>
/// <code>
/// #if DEBUG
/// _fallback = WpfUiFallback.Enable(client);   // _fallback.Dispose() 注销全部兜底工具
/// #endif
/// </code>
/// </example>
public sealed class WpfUiFallback : IDisposable
{
    private readonly ToolScope _scope;
    private readonly List<ToolRegistration> _tools = [];
    private bool _enabled;
    private bool _disposed;

    internal WpfUiInspector Inspector { get; }

    private WpfUiFallback(ToolScope scope, WpfUiFallbackOptions options)
    {
        _scope = scope;
        Inspector = new WpfUiInspector(options.Prefix, Math.Max(1, options.MaxItems), options.SettleDelay, WpfViewTools.DeclaredTools);
    }

    /// <summary>在客户端根作用域下建子作用域 <c>&lt;prefix&gt;-fallback</c> 并注册兜底工具。</summary>
    public static WpfUiFallback Enable(AppMcpClient client, WpfUiFallbackOptions? options = null)
    {
        ArgumentNullException.ThrowIfNull(client);
        options ??= new WpfUiFallbackOptions();
        return Start(client.CreateScope($"{options.Prefix}-fallback"), options);
    }

    /// <summary>在 <paramref name="parent"/> 下建子作用域 <c>&lt;prefix&gt;-fallback</c> 并注册兜底工具。</summary>
    public static WpfUiFallback Enable(ToolScope parent, WpfUiFallbackOptions? options = null)
    {
        ArgumentNullException.ThrowIfNull(parent);
        options ??= new WpfUiFallbackOptions();
        return Start(parent.CreateScope($"{options.Prefix}-fallback"), options);
    }

    private static WpfUiFallback Start(ToolScope scope, WpfUiFallbackOptions options)
    {
        var fallback = new WpfUiFallback(scope, options);
        try
        {
            fallback.Register(options.Prefix);
            WindowTracker.Changed += fallback.Recompute;
            fallback.Recompute();
            return fallback;
        }
        catch
        {
            fallback.Dispose();
            throw;
        }
    }

    /// <summary>兜底工具当前是否启用（应用有可见窗口）。</summary>
    public bool Enabled => !_disposed && _enabled;

    private void Recompute()
    {
        if (_disposed) return;
        var visible = WindowTracker.AnyVisible();
        if (visible == _enabled) return;
        _enabled = visible;
        foreach (var t in _tools) t.SetEnabled(visible);
        if (!visible) Inspector.Clear();
    }

    private Func<JsonElement, ToolContext, Task<object?>> OnUi(Func<JsonElement, Task<object?>> run) => async (args, _) =>
    {
        var dispatcher = Application.Current?.Dispatcher ?? throw new ToolCallException(ToolErrorKind.ToolDisabled, "WPF 应用尚未启动或已退出");
        if (dispatcher.CheckAccess()) return await Guarded(run, args);
        return await dispatcher.InvokeAsync(() => Guarded(run, args)).Task.Unwrap();
    };

    private Task<object?> Guarded(Func<JsonElement, Task<object?>> run, JsonElement args)
    {
        if (!Enabled) throw new ToolCallException(ToolErrorKind.ToolDisabled, "应用当前没有可见窗口，兜底工具不可用");
        return run(args);
    }

    private static Task<object?> Done(object result) => Task.FromResult<object?>(result);

    private void Register(string prefix)
    {
        ToolRegistration Add(string name, string title, string description, string schema, bool readOnly, Func<JsonElement, Task<object?>> run) =>
            _scope.RegisterTool($"{prefix}.{name}", description, OnUi(run), new ToolOptions
            {
                Title = title,
                InputSchemaJson = schema,
                Risk = readOnly ? ToolRisk.Read : ToolRisk.Write,
                Annotations = new ToolAnnotations { Title = title, ReadOnlyHint = readOnly },
                Surface = ToolSurface.View,
                Enabled = false,
            });

        var ui = Inspector;
        _tools.Add(Add("outline", "界面控件大纲",
            "兜底能力：列出当前窗口可见的可交互控件（按钮、输入框、复选框等），每行一个，带引用 eN，按窗口 / 对话框 / 分组归类。" +
            "应用已有对应的业务工具或标注 [已声明：…] 时请优先使用那些工具。引用用于 click / fill / press / scroll / read。",
            UiFallbackInput.Schemas.Outline(ui.MaxItems), readOnly: true,
            a => Done(ui.Outline(UiFallbackInput.String(a, "query"), UiFallbackInput.Ref(a, "within", required: false), UiFallbackInput.Int(a, "limit")))));
        _tools.Add(Add("click", "点击控件",
            $"兜底能力：激活 {prefix}.outline 中的控件（点击按钮、切换复选框、选中选项、展开 / 收起），返回界面变化摘要。",
            UiFallbackInput.Schemas.Ref, readOnly: false,
            async a => await ui.Click(UiFallbackInput.Ref(a, "ref")!)));
        _tools.Add(Add("fill", "填写控件",
            "兜底能力：填写文本框（会写回数据绑定）；复选框 / 单选框传 true / false；下拉框传选项文本；滑块传数字。密码类控件不支持。返回界面变化摘要。",
            UiFallbackInput.Schemas.Fill, readOnly: false,
            async a => await ui.Fill(UiFallbackInput.Ref(a, "ref")!, UiFallbackInput.Value(a))));
        _tools.Add(Add("press", "按键",
            "兜底能力：按键（ref 缺省为当前焦点控件）：Enter（激活 / 默认按钮）、Escape（取消按钮 / 关闭）、Tab / Shift+Tab（移动焦点）、Space（激活）。" +
            "输入文本请用 fill。返回界面变化摘要。",
            UiFallbackInput.Schemas.Press, readOnly: false,
            async a => await ui.Press(UiFallbackInput.Ref(a, "ref", required: false),
                UiFallbackInput.String(a, "key") ?? throw UiFallbackInput.Invalid("缺少参数 key"))));
        _tools.Add(Add("scroll", "滚动",
            "兜底能力：无 direction 时把控件滚动到可见；direction 为 up / down / left / right 时滚动该控件所在的滚动区一页（down = 向下翻看更多内容）。返回界面变化摘要。",
            UiFallbackInput.Schemas.Scroll, readOnly: false,
            async a => await ui.Scroll(UiFallbackInput.Ref(a, "ref")!, UiFallbackInput.String(a, "direction"))));
        _tools.Add(Add("read", "读取控件文本",
            $"兜底能力：读取控件的可见文本（折叠空白，默认最多 {UiOutlineFormat.ReadDefault} 字）；ref 缺省为全部窗口。",
            UiFallbackInput.Schemas.Read, readOnly: true,
            a => Done(ui.Read(UiFallbackInput.Ref(a, "ref", required: false), UiFallbackInput.Int(a, "maxChars")))));
    }

    /// <summary>注销全部兜底工具（幂等）。</summary>
    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;
        WindowTracker.Changed -= Recompute;
        Inspector.Clear();
        _tools.Clear();
        _scope.Dispose();
    }

    /// <summary>跟踪应用窗口的出现、显示 / 隐藏、最小化与关闭（类处理器只能注册一次，进程内共用）。</summary>
    /// <remarks>@invariant 只在 UI 线程上访问（Enable / Dispose 与窗口事件都在 UI 线程）。</remarks>
    private static class WindowTracker
    {
        private static readonly HashSet<Window> Tracked = [];
        private static bool _registered;

        private static Action? _handlers;

        public static event Action? Changed
        {
            add
            {
                EnsureRegistered();
                _handlers += value;
            }
            remove => _handlers -= value;
        }

        private static void EnsureRegistered()
        {
            if (_registered) return;
            _registered = true;
            EventManager.RegisterClassHandler(typeof(Window), FrameworkElement.LoadedEvent, new RoutedEventHandler((s, _) => Track((Window)s)));
            if (Application.Current is { } app)
            {
                foreach (Window w in app.Windows) Track(w);
            }
        }

        private static void Track(Window w)
        {
            if (!Tracked.Add(w)) return;
            w.IsVisibleChanged += OnVisibleChanged;
            w.StateChanged += OnChanged;
            w.Closed += OnClosed;
            Raise();
        }

        private static void OnVisibleChanged(object sender, DependencyPropertyChangedEventArgs e) => Raise();
        private static void OnChanged(object? sender, EventArgs e) => Raise();

        private static void OnClosed(object? sender, EventArgs e)
        {
            if (sender is Window w)
            {
                w.IsVisibleChanged -= OnVisibleChanged;
                w.StateChanged -= OnChanged;
                w.Closed -= OnClosed;
                Tracked.Remove(w);
            }
            Raise();
        }

        private static void Raise() => _handlers?.Invoke();

        public static bool AnyVisible()
        {
            if (Application.Current is not { } app) return false;
            foreach (Window w in app.Windows)
            {
                if (w.IsVisible && w.WindowState != WindowState.Minimized) return true;
            }
            return false;
        }
    }
}
