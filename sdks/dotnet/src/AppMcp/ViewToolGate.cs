namespace AppMcp;

/// <summary>
/// view 工具的可见性门控（spec/protocol.md 3.4、第 4c 项 E）：只有所在界面"可见"且"处于最上层（窗口激活、未被模态遮挡）"
/// 时启用其下的工具，否则禁用（禁用的工具不同步给 Host，Host 收到 <c>tools/changed</c>）。
/// </summary>
/// <remarks>
/// 与 UI 框架无关；WPF / WinUI 的事件绑定见 <c>AppMcp.Wpf</c> / <c>AppMcp.WinUI</c>。状态只能在 UI 线程上修改（与触发它的界面事件同线程）。
/// <see cref="Dispose"/> 禁用全部工具并停止跟踪（不注销）。
/// </remarks>
public sealed class ViewToolGate : IDisposable
{
    private readonly List<Action<bool>> _targets = new();
    private bool _visible;
    private bool _active;
    private bool _disposed;

    /// <param name="visible">初始可见性。</param>
    /// <param name="active">初始是否处于最上层（窗口激活）。</param>
    public ViewToolGate(bool visible = false, bool active = true)
    {
        _visible = visible;
        _active = active;
    }

    /// <summary>当前是否启用其下的工具。</summary>
    public bool Enabled => !_disposed && _visible && _active;

    /// <summary>跟踪一个工具，并立即按当前状态启用 / 禁用。</summary>
    public ViewToolGate Add(ToolRegistration tool)
    {
        ArgumentNullException.ThrowIfNull(tool);
        return Add(tool.SetEnabled);
    }

    /// <summary>跟踪任意"启用 / 禁用"动作（如一组工具、一个作用域的注册与注销）。</summary>
    public ViewToolGate Add(Action<bool> setEnabled)
    {
        ArgumentNullException.ThrowIfNull(setEnabled);
        ObjectDisposedException.ThrowIf(_disposed, this);
        _targets.Add(setEnabled);
        setEnabled(Enabled);
        return this;
    }

    /// <summary>界面是否可见（WPF <c>IsVisible</c>、WinUI 加载且 <c>Visibility.Visible</c>）。</summary>
    public void SetVisible(bool visible) => Apply(visible, _active);

    /// <summary>界面是否处于最上层（窗口激活、未被模态对话框遮挡）。</summary>
    public void SetActive(bool active) => Apply(_visible, active);

    private void Apply(bool visible, bool active)
    {
        if (_disposed) return;
        var before = Enabled;
        _visible = visible;
        _active = active;
        if (Enabled != before) Broadcast(Enabled);
    }

    private void Broadcast(bool enabled)
    {
        foreach (var t in _targets)
        {
            try { t(enabled); }
            catch (ObjectDisposedException) { } // @why 工具已被 Dispose（注销）：不再需要门控
            catch (AppMcpException) { }         // @why 客户端已停止 / 工具已注销：状态无从同步，忽略
        }
    }

    public void Dispose()
    {
        if (_disposed) return;
        var wasEnabled = Enabled;
        _disposed = true;
        if (wasEnabled) Broadcast(false);
        _targets.Clear();
    }
}
