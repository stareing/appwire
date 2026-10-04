using System.Text.Json;
using AppMcp.Native;

namespace AppMcp.Internal;

/// <summary>工具 handler 的原始形式：参数 JSON → 结果。</summary>
internal delegate Task<ToolOutcome> RawToolHandler(string argumentsJson, ToolContext context);

/// <summary>handler 的结果：返回值 JSON（null 表示 JSON null）+ 结构化结果（普通返回值时为 null）+ 撤销参数 JSON
/// （<see cref="ToolResult.Undo"/> 的参数；null 表示 <c>{}</c> 或无撤销信息）。</summary>
internal readonly record struct ToolOutcome(string? DataJson, ToolResult? Structured, string? UndoArgumentsJson = null)
{
    /// <summary>按运行时类型序列化；<see cref="ToolResult"/> 取其 Data 与撤销参数序列化并保留其余字段。</summary>
    public static ToolOutcome From(object? result, JsonSerializerOptions json) => result switch
    {
        null => default,
        ToolResult r => new(Serialize(r.Data, json), r, Serialize(r.Undo?.Arguments, json)),
        _ => new(Serialize(result, json), null),
    };

    private static string? Serialize(object? value, JsonSerializerOptions json) =>
        value is null ? null : JsonSerializer.Serialize(value, value.GetType(), json);
}

/// <summary>资源读取的原始形式：返回内容 JSON。</summary>
internal delegate Task<string> RawResourceReader(CancellationToken cancellationToken);

/// <summary>一次调用的原始 AmCall*，保证恰好完成一次。</summary>
internal sealed unsafe class PendingCall
{
    private readonly object _gate = new();
    private nint _call;
    private readonly CancellationTokenSource _cts = new();
    private readonly SynchronizationContext? _dispatcher;

    public PendingCall(nint call, SynchronizationContext? dispatcher)
    {
        _call = call;
        _dispatcher = dispatcher;
    }

    public CancellationToken Token => _cts.Token;
    public CancelReason? CancelReason { get; private set; }

    /// <summary>在分发线程上被调用：切到调度器 / 线程池后再触发令牌，避免在分发线程上执行用户回调。</summary>
    public void OnCancelled(CancelReason reason)
    {
        CancelReason = reason;
        Dispatch.Run(_dispatcher, () =>
        {
            try { _cts.Cancel(); } catch { }
        });
    }

    /// <summary>取走原始指针（只能成功一次）。与 <see cref="Hold"/> 互斥，保证 hold 不会用到已被消费的指针。</summary>
    private nint Take()
    {
        lock (_gate)
        {
            var call = _call;
            _call = 0;
            return call;
        }
    }

    /// <summary>延长持有（v3）：调用完成后仍阻止自动休眠，直到返回的对象被释放。</summary>
    public IDisposable Hold()
    {
        lock (_gate)
        {
            if (_call == 0) throw new AppMcpException(AppMcpStatus.AlreadyCompleted, "调用已完成");
            NativeMethods.Check(NativeMethods.am_call_hold(_call, out var raw));
            return new HoldRelease(new HoldSafeHandle(raw));
        }
    }

    /// <summary>报告进度（v10）。调用已完成、已取消或未连接时无副作用。与 <see cref="Take"/> 互斥，不会用到已被消费的指针。</summary>
    public void Progress(double progress, double? total, string? message)
    {
        lock (_gate)
        {
            if (_call == 0 || Token.IsCancellationRequested) return;
            using var strings = new Utf8Strings();
            var msg = message is null ? null : strings.AddPtr(message);
            // 调用刚被取消：原生层返回 AlreadyCompleted，进度只是提示，忽略。
            _ = NativeMethods.am_call_progress(_call, progress, total ?? -1.0, msg);
        }
    }

    public void Complete(ToolOutcome outcome, IReadOnlyList<string> stateHints)
    {
        var call = Take();
        if (call == 0) return;
        using var strings = new Utf8Strings();
        var structured = outcome.Structured;
        var allHints = structured?.StateHints is { Count: > 0 } extra ? stateHints.Concat(extra).ToArray() : stateHints;
        var hints = new nint[allHints.Count];
        for (var i = 0; i < hints.Length; i++) hints[i] = strings.Add(allHints[i]);
        AmStatus status;
        fixed (nint* p = hints)
        {
            var hintsPtr = hints.Length == 0 ? null : (byte**)p;
            if (structured is null)
            {
                status = NativeMethods.am_call_complete(call, strings.AddPtr(outcome.DataJson ?? "null"), hintsPtr, (nuint)hints.Length);
            }
            else
            {
                var result = new AmCallResult
                {
                    StructSize = (uint)sizeof(AmCallResult),
                    DataJson = strings.Add(outcome.DataJson),
                    StateHints = (nint)hintsPtr,
                    StateHintsLen = (nuint)hints.Length,
                    Status = (int)structured.Status,
                    StateResource = strings.Add(structured.StateResource),
                    Summary = strings.Add(structured.Summary),
                    AnnotationsJson = strings.Add(AnnotationsJson.Serialize(structured.Annotations)),
                    UndoTool = strings.Add(structured.Undo?.Tool),
                    UndoArgumentsJson = strings.Add(outcome.UndoArgumentsJson),
                    UndoLabel = strings.Add(structured.Undo?.Label),
                };
                status = NativeMethods.am_call_complete_ex(call, &result);
            }
        }
        if (status == AmStatus.InvalidJson)
        {
            // 未被消费：改为失败完成。
            FailRaw(call, "HANDLER_ERROR", "handler 返回的结果不是合法的 JSON：" + NativeMethods.LastError());
        }
    }

    public void Fail(string kind, string message)
    {
        var call = Take();
        if (call == 0) return;
        FailRaw(call, kind, message);
    }

    /// <summary>失败完成并附带结构化详情（JSON 文本）。详情非法时退化为不带详情的失败。</summary>
    public void FailWithDetails(string kind, string message, string? detailsJson)
    {
        var call = Take();
        if (call == 0) return;
        using var strings = new Utf8Strings();
        var status = NativeMethods.am_call_fail_with_details(call, strings.AddPtr(kind), strings.AddPtr(message), strings.AddPtr(detailsJson));
        if (status == AmStatus.InvalidJson) FailRaw(call, kind, message);
    }

    /// <summary>以 USER_ACTION_REQUIRED 失败完成（v11）；reason / uri 为 null 时不出现在错误的 data 中。</summary>
    public void FailUserAction(string message, string? reason, string? uri)
    {
        var call = Take();
        if (call == 0) return;
        using var strings = new Utf8Strings();
        NativeMethods.am_call_fail_user_action(call, strings.AddPtr(message), strings.AddPtr(reason), strings.AddPtr(uri));
    }

    internal static void FailRaw(nint call, string kind, string message)
    {
        using var strings = new Utf8Strings();
        NativeMethods.am_call_fail(call, strings.AddPtr(kind), strings.AddPtr(message));
    }
}

internal sealed unsafe class PendingRead
{
    private nint _read;
    private int _done;

    public PendingRead(nint read) => _read = read;

    public void Complete(string contentsJson)
    {
        if (Interlocked.Exchange(ref _done, 1) != 0) return;
        var read = _read;
        _read = 0;
        using var strings = new Utf8Strings();
        var status = NativeMethods.am_read_complete(read, strings.AddPtr(contentsJson));
        if (status == AmStatus.InvalidJson)
        {
            FailRaw(read, "HANDLER_ERROR", "reader 返回的内容不是合法的 JSON：" + NativeMethods.LastError());
        }
    }

    public void Fail(string kind, string message)
    {
        var read = Take();
        if (read == 0) return;
        FailRaw(read, kind, message);
    }

    /// <summary>失败完成并附带结构化详情（JSON 文本，v12）。详情非法时退化为不带详情的失败。</summary>
    public void FailWithDetails(string kind, string message, string? detailsJson)
    {
        var read = Take();
        if (read == 0) return;
        using var strings = new Utf8Strings();
        var status = NativeMethods.am_read_fail_with_details(read, strings.AddPtr(kind), strings.AddPtr(message), strings.AddPtr(detailsJson));
        if (status == AmStatus.InvalidJson) FailRaw(read, kind, message);
    }

    /// <summary>以 USER_ACTION_REQUIRED 失败完成（v12）；reason / uri 为 null 时不出现在错误的 data 中。</summary>
    public void FailUserAction(string message, string? reason, string? uri)
    {
        var read = Take();
        if (read == 0) return;
        using var strings = new Utf8Strings();
        NativeMethods.am_read_fail_user_action(read, strings.AddPtr(message), strings.AddPtr(reason), strings.AddPtr(uri));
    }

    private nint Take()
    {
        if (Interlocked.Exchange(ref _done, 1) != 0) return 0;
        var read = _read;
        _read = 0;
        return read;
    }

    internal static void FailRaw(nint read, string kind, string message)
    {
        using var strings = new Utf8Strings();
        NativeMethods.am_read_fail(read, strings.AddPtr(kind), strings.AddPtr(message));
    }
}

/// <summary>工具 handler 的 user_data 目标。</summary>
internal sealed class ToolInvoker(RawToolHandler handler, SynchronizationContext? dispatcher, JsonSerializerOptions? json = null)
{
    /// <summary>在分发线程上被调用：读取调用信息、设置取消回调，然后把 handler 投递到调度器。</summary>
    public void Invoke(nint call)
    {
        var pending = new PendingCall(call, dispatcher);
        var id = NativeMethods.PtrToString(NativeMethods.am_call_id(call)) ?? string.Empty;
        var name = NativeMethods.PtrToString(NativeMethods.am_call_tool_name(call)) ?? string.Empty;
        var args = NativeMethods.PtrToString(NativeMethods.am_call_arguments_json(call)) ?? "{}";
        var idempotencyKey = NativeMethods.PtrToString(NativeMethods.am_call_idempotency_key(call));

        // user_data 的所有权交给库（失败时库会立即调用 FreeGCHandle）。
        NativeMethods.am_call_set_cancel_callback(call, Callbacks.CancelPtr, Callbacks.Alloc(pending), Callbacks.FreeGCHandlePtr);

        var context = new ToolContext(id, name, pending, idempotencyKey);
        Dispatch.Run(dispatcher, () => RunAsync(pending, args, context));
    }

    private async Task RunAsync(PendingCall pending, string args, ToolContext context)
    {
        try
        {
            var result = await handler(args, context).ConfigureAwait(false);
            pending.Complete(result, context.StateHints);
        }
        catch (UserActionRequiredException e)
        {
            pending.FailUserAction(e.Message, e.Reason, e.Uri);
        }
        catch (ToolCallException e) when (e.Details is not null)
        {
            pending.FailWithDetails(e.Kind.ToProtocolString(), e.Message, ErrorDetails.Serialize(e.Details, json));
        }
        catch (ToolCallException e)
        {
            pending.Fail(e.Kind.ToProtocolString(), e.Message);
        }
        catch (OperationCanceledException) when (context.CancellationToken.IsCancellationRequested)
        {
            pending.Fail("CANCELLED", "调用已取消");
        }
        catch (Exception e)
        {
            pending.Fail("HANDLER_ERROR", e.Message);
        }
    }
}

/// <summary><see cref="ToolCallException.Details"/> 的 JSON 文本（调用与读取共用）。</summary>
internal static class ErrorDetails
{
    /// <summary>@error 无法序列化时返回 null（调用方退化为不带详情的失败）。</summary>
    public static string? Serialize(object details, JsonSerializerOptions? json)
    {
        try { return JsonSerializer.Serialize(details, details.GetType(), json ?? JsonSerializerOptions.Web); }
        catch (Exception) { return null; }
    }
}

internal sealed class ResourceInvoker(RawResourceReader reader, SynchronizationContext? dispatcher, JsonSerializerOptions? json = null)
{
    public void Invoke(nint read)
    {
        var pending = new PendingRead(read);
        Dispatch.Run(dispatcher, () => RunAsync(pending));
    }

    private async Task RunAsync(PendingRead pending)
    {
        try
        {
            var contents = await reader(CancellationToken.None).ConfigureAwait(false);
            pending.Complete(contents);
        }
        catch (UserActionRequiredException e)
        {
            pending.FailUserAction(e.Message, e.Reason, e.Uri);
        }
        catch (ToolCallException e) when (e.Details is not null)
        {
            pending.FailWithDetails(e.Kind.ToProtocolString(), e.Message, ErrorDetails.Serialize(e.Details, json));
        }
        catch (ToolCallException e)
        {
            pending.Fail(e.Kind.ToProtocolString(), e.Message);
        }
        catch (Exception e)
        {
            pending.Fail("HANDLER_ERROR", e.Message);
        }
    }
}

/// <summary>持有的释放句柄（<see cref="AppMcpClient.Hold"/>、<see cref="ToolContext.Hold"/> 返回）。Dispose 幂等。</summary>
internal sealed class HoldRelease(HoldSafeHandle handle) : IDisposable
{
    public void Dispose() => handle.Dispose();
}

/// <summary><see cref="AppMcpClient.Busy"/> 的作用域：首次 Dispose 时归还一次计数，之后无效果。</summary>
internal sealed class BusyRelease(BusyState state) : IDisposable
{
    private int _released;

    /// <remarks>@error 客户端已释放 / 停止时忽略：Dispose 不应抛出，且停止后的客户端不再执行调用。</remarks>
    public void Dispose()
    {
        if (Interlocked.Exchange(ref _released, 1) != 0) return;
        try
        {
            state.Exit();
        }
        catch (AppMcpException)
        {
        }
        catch (ObjectDisposedException)
        {
        }
    }
}
