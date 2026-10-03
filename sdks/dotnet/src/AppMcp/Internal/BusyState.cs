namespace AppMcp.Internal;

/// <summary>合并 <see cref="AppMcpClient.SetBusy"/> 显式开关与 <see cref="AppMcpClient.Busy"/> 作用域计数（spec/protocol.md 5.3）。</summary>
/// <remarks>
/// @invariant 交给原生库的值 = 开关 ∨ 作用域数 &gt; 0；开关与作用域互不清除；只在有效值变化时调用 <c>apply</c>。
/// @side-effect <c>apply</c> 在锁内调用，下发顺序与状态变化顺序一致；下发失败时异常传给调用方，已下发值不变（下次变化重试）。
/// </remarks>
internal sealed class BusyState(Action<bool> apply)
{
    private readonly Lock _gate = new();
    private bool _manual;
    private int _scopes;
    private bool _applied;

    public bool Effective
    {
        get
        {
            lock (_gate) return _manual || _scopes > 0;
        }
    }

    public void Set(bool busy)
    {
        lock (_gate)
        {
            _manual = busy;
            Push();
        }
    }

    public void Enter()
    {
        lock (_gate)
        {
            _scopes++;
            try
            {
                Push();
            }
            catch
            {
                _scopes--;
                throw;
            }
        }
    }

    public void Exit()
    {
        lock (_gate)
        {
            _scopes--;
            Push();
        }
    }

    private void Push()
    {
        var effective = _manual || _scopes > 0;
        if (effective == _applied) return;
        apply(effective);
        _applied = effective;
    }
}
