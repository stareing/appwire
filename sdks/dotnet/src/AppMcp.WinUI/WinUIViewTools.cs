using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace AppMcp.WinUI;

/// <summary>
/// WinUI 3 的 view 工具绑定（spec/protocol.md 3.4、第 4c 项 E）：控件已加载、<see cref="UIElement.Visibility"/> 为 Visible
/// 且所在窗口已激活时启用工具，否则禁用。WinUI 没有 IsVisible：以 Loaded / Unloaded 与 Visibility 变化近似（祖先折叠不可知）。
/// </summary>
public static class WinUIViewTools
{
    /// <summary>把工具绑定到控件与窗口。返回值 Dispose 时解除绑定并禁用工具（不注销）。必须在 UI 线程上调用。</summary>
    /// <param name="element">工具所在界面（页面或控件）。</param>
    /// <param name="window">控件所在窗口（WinUI 3 无法由控件反查 <see cref="Window"/>）；为 null 时只看控件。</param>
    /// <param name="tools">要门控的 view 工具。</param>
    public static IDisposable Bind(FrameworkElement element, Window? window, params ToolRegistration[] tools)
    {
        ArgumentNullException.ThrowIfNull(element);
        var gate = new ViewToolGate(IsShown(element), active: true);
        foreach (var t in tools) gate.Add(t);
        return new ElementBinding(element, window, gate);
    }

    private static bool IsShown(FrameworkElement e) => e.IsLoaded && e.Visibility == Visibility.Visible;

    private sealed class ElementBinding : IDisposable
    {
        private readonly FrameworkElement _element;
        private readonly Window? _window;
        private readonly ViewToolGate _gate;
        private readonly long _visibilityToken;

        public ElementBinding(FrameworkElement element, Window? window, ViewToolGate gate)
        {
            _element = element;
            _window = window;
            _gate = gate;
            element.Loaded += OnLoadedChanged;
            element.Unloaded += OnLoadedChanged;
            _visibilityToken = element.RegisterPropertyChangedCallback(UIElement.VisibilityProperty, (_, _) => Refresh());
            if (window is not null) window.Activated += OnActivated;
        }

        private void Refresh() => _gate.SetVisible(IsShown(_element));
        private void OnLoadedChanged(object sender, RoutedEventArgs e) => Refresh();
        private void OnActivated(object sender, WindowActivatedEventArgs e) =>
            _gate.SetActive(e.WindowActivationState != WindowActivationState.Deactivated);

        public void Dispose()
        {
            _element.Loaded -= OnLoadedChanged;
            _element.Unloaded -= OnLoadedChanged;
            _element.UnregisterPropertyChangedCallback(UIElement.VisibilityProperty, _visibilityToken);
            if (_window is not null) _window.Activated -= OnActivated;
            _gate.Dispose();
        }
    }
}

/// <summary>
/// WinUI 3 <see cref="Frame"/> 的导航适配：页面名 → 页面类型，<see cref="Frame.Navigate(Type, object)"/> 的参数为
/// <see cref="NavigationRequest"/>（页面在 OnNavigatedTo 中读取）；等目标页面 Loaded（其 view 工具已注册 / 启用）后再完成。
/// </summary>
public static class WinUINavigation
{
    /// <summary>未列出的页面以 NAVIGATION_FAILED 失败。handler 应在客户端调度器（UI 线程，WinUI 的 DispatcherQueue 同步上下文）上运行。</summary>
    public static Func<NavigationRequest, Task> ForFrame(Frame frame, IReadOnlyDictionary<string, Type> pages)
    {
        ArgumentNullException.ThrowIfNull(frame);
        ArgumentNullException.ThrowIfNull(pages);
        return async request =>
        {
            if (!pages.TryGetValue(request.Page, out var type)) throw new InvalidOperationException($"没有页面「{request.Page}」");
            if (!frame.DispatcherQueue.HasThreadAccess) throw new InvalidOperationException("导航回调必须在 UI 线程上运行（在 UI 线程上创建 AppMcpClient）");
            if (!frame.Navigate(type, request)) throw new InvalidOperationException($"导航到「{request.Page}」失败");
            if (frame.Content is FrameworkElement { IsLoaded: false } page)
            {
                var loaded = new TaskCompletionSource();
                void OnLoaded(object s, RoutedEventArgs e) => loaded.TrySetResult();
                page.Loaded += OnLoaded;
                try { await loaded.Task; }
                finally { page.Loaded -= OnLoaded; }
            }
        };
    }
}
