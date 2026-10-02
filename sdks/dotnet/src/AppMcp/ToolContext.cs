using AppMcp.Internal;

namespace AppMcp;

/// <summary>一次工具调用的上下文。</summary>
public sealed class ToolContext
{
    private readonly PendingCall _pending;
    private readonly List<string> _stateHints = new();

    internal ToolContext(string callId, string toolName, PendingCall pending, string? idempotencyKey = null)
    {
        CallId = callId;
        ToolName = toolName;
        IdempotencyKey = idempotencyKey;
        _pending = pending;
    }

    public string CallId { get; }
    public string ToolName { get; }

    /// <summary>
    /// Agent 给出的幂等键（原样，spec/protocol.md 3.3「idempotencyKey」）；没有时为 null。
    /// App 自行决定如何使用（如作为业务去重键、传给后端）；同一工具同一键的重复调用已由核心按首次结果重放。
    /// </summary>
    public string? IdempotencyKey { get; }

    /// <summary>Host 取消、超时、断线或客户端停止时触发。</summary>
    public CancellationToken CancellationToken => _pending.Token;

    /// <summary>取消原因（未取消时为 null）。</summary>
    public CancelReason? CancelReason => _pending.CancelReason;

    /// <summary>
    /// 延长持有：调用完成后仍阻止自动休眠（handler 发起的长任务），直到返回的对象被释放。
    /// 必须在调用完成（handler 返回）之前调用；调用进行中本身已视为非空闲，无需手动持有。
    /// </summary>
    public IDisposable Hold() => _pending.Hold();

    /// <summary>
    /// 报告进度（spec/protocol.md 3.3）：Host 合并后转发给 Agent（MCP notifications/progress）。
    /// <paramref name="progress"/> 应递增（不递增的值被 Host 丢弃），<paramref name="total"/> 未知时为 null。
    /// 调用已结束、已取消或未连接时无副作用。
    /// </summary>
    public void Progress(double progress, double? total = null, string? message = null) => _pending.Progress(progress, total, message);

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
