using System.Windows;
using System.Windows.Controls;
using System.Windows.Threading;

namespace AppMcp.Wpf;

/// <summary>
/// WPF 的 view 工具绑定（spec/protocol.md 3.4、第 4c 项 E）：控件 <c>IsVisible</c> 为 true 且所在窗口已激活时启用工具，
/// 否则禁用（控件卸载、切到其他页面、窗口失去激活 / 被模态对话框遮挡）。
/// </summary>
/// <example>
/// <code>
/// var checkout = client.RegisterTool("cart.checkout", "结算", Checkout, new ToolOptions { Surface = ToolSurface.View, Page = "cart" });
/// _binding = WpfViewTools.Bind(cartPage, checkout);   // 页面卸载 / 不可见 / 窗口失活时自动禁用
/// </code>
/// </example>
public static class WpfViewTools
{
    /// <summary>把工具绑定到控件的可见性与所在窗口的激活状态。返回值 Dispose 时解除绑定并禁用工具（不注销）。</summary>
    /// <remarks>必须在 UI 线程上调用。窗口激活状态取 <see cref="Window.IsActive"/>；控件换窗口（重新 Loaded）时重新订阅。</remarks>
    public static IDisposable Bind(FrameworkElement element, params ToolRegistration[] tools)
    {
        ArgumentNullException.ThrowIfNull(element);
        var gate = new ViewToolGate(element.IsVisible, Window.GetWindow(element)?.IsActive ?? true);
        foreach (var t in tools) gate.Add(t);
        Declare(element, tools.Select(t => t.Name));
        return new ElementBinding(element, gate, tools.Select(t => t.Name).ToList());
    }

    /// <summary>控件 → 绑定到它的工具名：控件兜底（<see cref="UiFallback.WpfUiFallback"/>）据此在大纲中标出 <c>[已声明：…]</c>。</summary>
    private static readonly System.Runtime.CompilerServices.ConditionalWeakTable<FrameworkElement, List<string>> Declared = new();

    private static void Declare(FrameworkElement element, IEnumerable<string> names)
    {
        var list = Declared.GetOrCreateValue(element);
        lock (list) list.AddRange(names.Where(n => !list.Contains(n)));
    }

    private static void Undeclare(FrameworkElement element, IReadOnlyCollection<string> names)
    {
        if (!Declared.TryGetValue(element, out var list)) return;
        lock (list) list.RemoveAll(names.Contains);
    }

    /// <summary>绑定到该控件的工具名（多个以 ", " 连接）；没有时为 null。</summary>
    internal static string? DeclaredTools(UIElement element)
    {
        if (element is not FrameworkElement fe || !Declared.TryGetValue(fe, out var list)) return null;
        lock (list) return list.Count == 0 ? null : string.Join(", ", list);
    }

    /// <summary>同 <see cref="Bind(FrameworkElement, ToolRegistration[])"/>，门控由调用方提供（可再 <see cref="ViewToolGate.Add(Action{bool})"/>）。</summary>
    public static IDisposable Bind(FrameworkElement element, ViewToolGate gate)
    {
        ArgumentNullException.ThrowIfNull(element);
        ArgumentNullException.ThrowIfNull(gate);
        gate.SetVisible(element.IsVisible);
        gate.SetActive(Window.GetWindow(element)?.IsActive ?? true);
        return new ElementBinding(element, gate);
    }

    private sealed class ElementBinding : IDisposable
    {
        private readonly FrameworkElement _element;
        private readonly ViewToolGate _gate;
        private readonly IReadOnlyCollection<string> _declared;
        private Window? _window;

        public ElementBinding(FrameworkElement element, ViewToolGate gate, IReadOnlyCollection<string>? declared = null)
        {
            _element = element;
            _gate = gate;
            _declared = declared ?? [];
            element.IsVisibleChanged += OnVisibleChanged;
            element.Loaded += OnLoaded;
            element.Unloaded += OnUnloaded;
            Attach(Window.GetWindow(element));
        }

        private void OnVisibleChanged(object sender, DependencyPropertyChangedEventArgs e) => _gate.SetVisible((bool)e.NewValue);

        private void OnLoaded(object sender, RoutedEventArgs e)
        {
            Attach(Window.GetWindow(_element));
            _gate.SetVisible(_element.IsVisible);
        }

        private void OnUnloaded(object sender, RoutedEventArgs e)
        {
            _gate.SetVisible(false);
            Attach(null);
        }

        private void Attach(Window? window)
        {
            if (ReferenceEquals(window, _window)) return;
            if (_window is not null)
            {
                _window.Activated -= OnActivated;
                _window.Deactivated -= OnDeactivated;
            }
            _window = window;
            if (window is null) return;
            window.Activated += OnActivated;
            window.Deactivated += OnDeactivated;
            _gate.SetActive(window.IsActive);
        }

        private void OnActivated(object? sender, EventArgs e) => _gate.SetActive(true);
        private void OnDeactivated(object? sender, EventArgs e) => _gate.SetActive(false);

        public void Dispose()
        {
            _element.IsVisibleChanged -= OnVisibleChanged;
            _element.Loaded -= OnLoaded;
            _element.Unloaded -= OnUnloaded;
            Attach(null);
            Undeclare(_element, _declared);
            _gate.Dispose();
        }
    }
}

/// <summary>
/// WPF <see cref="Frame"/> 的导航适配（<see cref="AppMcpClient.SetNavigationHandler(Func{NavigationRequest, Task}?)"/>）：
/// 按页面名创建内容并 <see cref="Frame.Navigate(object)"/>，等导航完成、新页面加载（其 view 工具已注册 / 启用）后再完成。
/// </summary>
/// <example>
/// <code>
/// client.SetNavigationHandler(WpfNavigation.ForFrame(MainFrame, new Dictionary&lt;string, Func&lt;NavigationRequest, object&gt;&gt;
/// {
///     ["cart"] = _ => new CartPage(client),
///     ["orders.detail"] = r => new OrderPage(client, r.Params?.GetProperty("id").GetString()),
/// }));
/// </code>
/// </example>
public static class WpfNavigation
{
    /// <summary>未列出的页面以 NAVIGATION_FAILED 失败；工厂抛出 <see cref="NavigationDeniedException"/> 即拒绝。handler 在客户端调度器（UI 线程）上运行。
    /// 所在窗口未激活（后台、最小化）时先还原并激活；系统不允许前置时以 USER_ACTION_REQUIRED（reason "foreground"）回复。</summary>
    public static Func<NavigationRequest, Task> ForFrame(Frame frame, IReadOnlyDictionary<string, Func<NavigationRequest, object>> pages)
    {
        ArgumentNullException.ThrowIfNull(frame);
        ArgumentNullException.ThrowIfNull(pages);
        return async request =>
        {
            if (!pages.TryGetValue(request.Page, out var create)) throw new InvalidOperationException($"没有页面「{request.Page}」");
            // 不同线程（客户端未在 UI 线程上创建）时切回 Frame 的 Dispatcher。
            await frame.Dispatcher.InvokeAsync(async () =>
            {
                BringToFront(Window.GetWindow(frame));
                var content = create(request);
                var navigated = new TaskCompletionSource();
                void OnNavigated(object s, System.Windows.Navigation.NavigationEventArgs e) => navigated.TrySetResult();
                void OnFailed(object s, System.Windows.Navigation.NavigationFailedEventArgs e)
                {
                    e.Handled = true;
                    navigated.TrySetException(e.Exception);
                }
                frame.Navigated += OnNavigated;
                frame.NavigationFailed += OnFailed;
                try
                {
                    if (!frame.Navigate(content)) throw new InvalidOperationException($"导航到「{request.Page}」被取消");
                    await navigated.Task;
                }
                finally
                {
                    frame.Navigated -= OnNavigated;
                    frame.NavigationFailed -= OnFailed;
                }
                // @why Loaded（及其中注册 / 启用 view 工具）在 Navigated 之后的布局阶段触发：让出到 Loaded 优先级之后再完成。
                await frame.Dispatcher.InvokeAsync(() => { }, DispatcherPriority.Loaded);
            }).Task.Unwrap();
        };
    }

    /// <summary>@why 后台导航（桌面默认 navigateInBackground = true）：view 工具只在窗口激活时启用，导航前先把窗口提到前台。</summary>
    /// <remarks>@error 系统前台锁拒绝激活时抛出 <see cref="UserActionRequiredException"/>（foreground）。</remarks>
    private static void BringToFront(Window? window)
    {
        if (window is null || window.IsActive) return;
        if (window.WindowState == WindowState.Minimized) window.WindowState = WindowState.Normal;
        if (!window.IsVisible) window.Show();
        if (!window.Activate()) throw new UserActionRequiredException("请把窗口切到前台后重试", UserActionReason.Foreground);
    }
}
