using AppMcp.Internal;

namespace AppMcp;

/// <summary>一次工具调用的上下文。</summary>
public sealed class ToolContext
{
    private readonly PendingCall _pending;
    private readonly List<string> _stateHints = new();

    internal ToolContext(string callId, string toolName, PendingCall pending)
    {
        CallId = callId;
        ToolName = toolName;
        _pending = pending;
    }

    public string CallId { get; }
    public string ToolName { get; }

    /// <summary>Host 取消、超时、断线或客户端停止时触发。</summary>
    public CancellationToken CancellationToken => _pending.Token;

    /// <summary>取消原因（未取消时为 null）。</summary>
    public CancelReason? CancelReason => _pending.CancelReason;

    /// <summary>
    /// 延长持有：调用完成后仍阻止自动休眠（handler 发起的长任务），直到返回的对象被释放。
    /// 必须在调用完成（handler 返回）之前调用；调用进行中本身已视为非空闲，无需手动持有。
    /// </summary>
    public IDisposable Hold() => _pending.Hold();

    /// <summary>添加状态提示（结果中的 stateHints，提示模型哪些状态已变化，如相关资源名）。</summary>
    public void AddStateHint(string hint)
    {
        lock (_stateHints) _stateHints.Add(hint);
    }

    internal IReadOnlyList<string> StateHints
    {
        get
        {
            lock (_stateHints) return _stateHints.ToArray();
        }
    }
}
