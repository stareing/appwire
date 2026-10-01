using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Text.Json;
using AppMcp.Hub.Native;

namespace AppMcp.Hub.Internal;

/// <summary>
/// 传给 C 接口的回调。user_data 为 <see cref="GCHandle"/>：一次性结果回调在回调内释放；
/// 常驻回调（事件、审批、配对）由库调用 free_user_data（<see cref="FreeGCHandle"/>）释放。
/// 回调在 Hub 的分发线程上执行，只做最少的工作并吞掉所有异常——异常不能跨越 FFI 边界。
/// </summary>
internal static unsafe class HubCallbacks
{
    internal static nint FreeGCHandlePtr => (nint)(delegate* unmanaged[Cdecl]<nint, void>)&FreeGCHandle;
    internal static nint ResultPtr => (nint)(delegate* unmanaged[Cdecl]<nint, nint, void>)&OnResult;
    internal static nint ProgressPtr => (nint)(delegate* unmanaged[Cdecl]<nint, nint, void>)&OnProgress;
    internal static nint EventPtr => (nint)(delegate* unmanaged[Cdecl]<nint, nint, void>)&OnEvent;
    internal static nint ApprovalPtr => (nint)(delegate* unmanaged[Cdecl]<nint, nint, nint, void>)&OnApproval;
    internal static nint PairingPtr => (nint)(delegate* unmanaged[Cdecl]<nint, nint, nint, void>)&OnPairing;
    internal static nint WakerPtr => (nint)(delegate* unmanaged[Cdecl]<nint, nint, nint, void>)&OnWake;

    internal static nint Alloc(object target) => GCHandle.ToIntPtr(GCHandle.Alloc(target));

    internal static void Free(nint userData)
    {
        if (userData != 0) GCHandle.FromIntPtr(userData).Free();
    }

    private static T? Target<T>(nint userData) where T : class =>
        userData == 0 ? null : GCHandle.FromIntPtr(userData).Target as T;

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    private static void FreeGCHandle(nint userData)
    {
        try { Free(userData); }
        catch { }
    }

    /// <summary>一次性结果：恰好调用一次，在此释放 GCHandle。</summary>
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    private static void OnResult(nint userData, nint json)
    {
        try
        {
            var text = HubNativeMethods.TakeString(json) ?? "null";
            var tcs = Target<TaskCompletionSource<string>>(userData);
            Free(userData);
            tcs?.TrySetResult(text);
        }
        catch
        {
        }
    }

    /// <summary>调用进度：与结果共用 user_data（<see cref="ProgressCallSink"/>），总在结果之前；不释放 GCHandle。</summary>
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    private static void OnProgress(nint userData, nint json)
    {
        try
        {
            var text = HubNativeMethods.TakeString(json);
            if (text is not null) Target<ProgressCallSink>(userData)?.Report(text);
        }
        catch
        {
        }
    }

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    private static void OnEvent(nint userData, nint json)
    {
        try
        {
            var text = HubNativeMethods.TakeString(json);
            if (text is not null) Target<HubEventSink>(userData)?.OnEvent(text);
        }
        catch
        {
        }
    }

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    private static void OnApproval(nint userData, nint json, nint approval)
    {
        try
        {
            var text = HubNativeMethods.TakeString(json) ?? "{}";
            if (Target<DecisionSink<ApprovalRequest>>(userData) is { } sink)
            {
                sink.Decide(text, approval);
                return;
            }
            HubNativeMethods.am_hub_approval_complete(approval, false);
        }
        catch
        {
            try { HubNativeMethods.am_hub_approval_complete(approval, false); } catch { }
        }
    }

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    private static void OnPairing(nint userData, nint json, nint pairing)
    {
        try
        {
            var text = HubNativeMethods.TakeString(json) ?? "{}";
            if (Target<DecisionSink<PairingRequest>>(userData) is { } sink)
            {
                sink.Decide(text, pairing);
                return;
            }
            HubNativeMethods.am_hub_pairing_complete(pairing, false);
        }
        catch
        {
            try { HubNativeMethods.am_hub_pairing_complete(pairing, false); } catch { }
        }
    }

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    private static void OnWake(nint userData, nint json, nint wake)
    {
        try
        {
            var text = HubNativeMethods.TakeString(json) ?? "{}";
            if (Target<WakeSink>(userData) is { } sink)
            {
                sink.Wake(text, wake);
                return;
            }
            WakeSink.Complete(wake, "LAUNCH_FAILED", "未设置 Waker");
        }
        catch
        {
            try { WakeSink.Complete(wake, "LAUNCH_FAILED", "Waker 回调失败"); } catch { }
        }
    }
}

/// <summary>唤醒：在调度器上运行 handler，结束后恰好完成一次原生句柄。</summary>
internal sealed class WakeSink(
    Func<WakeRequest, CancellationToken, Task> handler,
    SynchronizationContext? dispatcher,
    CancellationToken lifetime)
{
    public void Wake(string json, nint handle)
    {
        WakeRequest? request;
        try
        {
            request = JsonSerializer.Deserialize<WakeRequest>(json, AppMcpHub.WireOptions);
        }
        catch (JsonException)
        {
            request = null;
        }
        if (request is null)
        {
            Complete(handle, "LAUNCH_FAILED", "无法解析唤醒请求");
            return;
        }

        async Task Run()
        {
            string? kind = null;
            string? message = null;
            try
            {
                await handler(request, lifetime);
            }
            catch (WakeFailedException e)
            {
                kind = e.Kind;
                message = e.Message;
            }
            catch (Exception e)
            {
                kind = "LAUNCH_FAILED";
                message = e.Message;
            }
            finally
            {
                // 唤醒已超时或 Hub 已停止时返回 AlreadyCompleted，忽略即可。
                if (kind is null) Complete(handle, null, null);
                else Complete(handle, kind, message);
            }
        }

        if (dispatcher is null) _ = Task.Run(Run);
        else dispatcher.Post(static s => _ = ((Func<Task>)s!)(), (Func<Task>)Run);
    }

    /// <summary>kind 为 null 表示成功。</summary>
    internal static unsafe void Complete(nint handle, string? kind, string? message)
    {
        using var s = new Utf8Strings();
        HubNativeMethods.am_hub_waker_complete(handle, kind is null, s.Add(kind), s.Add(message));
    }
}

/// <summary>事件接收者。只弱引用 Hub，避免 GCHandle 让未 Dispose 的 Hub 永远无法回收。</summary>
internal sealed class HubEventSink(SynchronizationContext? dispatcher)
{
    private WeakReference<AppMcpHub>? _hub;

    public void Attach(AppMcpHub hub) => _hub = new WeakReference<AppMcpHub>(hub);

    public void OnEvent(string json)
    {
        HubEventArgs args;
        using (var doc = JsonDocument.Parse(json))
        {
            var root = doc.RootElement.Clone();
            var type = root.TryGetProperty("type", out var t) ? t.GetString() ?? string.Empty : string.Empty;
            args = new HubEventArgs(type, root);
        }
        if (_hub is null || !_hub.TryGetTarget(out var hub)) return;
        if (dispatcher is null)
        {
            // 未指定调度器：直接在分发线程上触发（保持事件顺序；订阅者必须尽快返回）。
            try { hub.RaiseEvent(args); } catch { }
            return;
        }
        dispatcher.Post(_ =>
        {
            if (_hub.TryGetTarget(out var h)) h.RaiseEvent(args);
        }, null);
    }
}

/// <summary>审批 / 配对：在调度器上运行 handler，结束后恰好完成一次原生句柄；异常与取消按拒绝处理。</summary>
internal sealed class DecisionSink<TRequest>(
    Func<TRequest, CancellationToken, Task<bool>> handler,
    SynchronizationContext? dispatcher,
    CancellationToken lifetime,
    Func<nint, bool, HubStatus> complete)
    where TRequest : class
{
    public void Decide(string json, nint handle)
    {
        TRequest? request;
        try
        {
            request = JsonSerializer.Deserialize<TRequest>(json, AppMcpHub.WireOptions);
        }
        catch (JsonException)
        {
            request = null;
        }
        if (request is null)
        {
            complete(handle, false);
            return;
        }

        async Task Run()
        {
            var approved = false;
            try
            {
                approved = await handler(request, lifetime);
            }
            catch
            {
                approved = false;
            }
            finally
            {
                // 超时或调用已取消时返回 AlreadyCompleted，忽略即可。
                complete(handle, approved);
            }
        }

        if (dispatcher is null) _ = Task.Run(Run);
        else dispatcher.Post(static s => _ = ((Func<Task>)s!)(), (Func<Task>)Run);
    }
}

/// <summary>带进度的调用：结果（<see cref="TaskCompletionSource{TResult}"/>）+ 进度接收者。</summary>
internal sealed class ProgressCallSink(IProgress<CallProgress> progress)
    : TaskCompletionSource<string>(TaskCreationOptions.RunContinuationsAsynchronously)
{
    /// <summary>progress_json → <see cref="CallProgress"/>；无法解析时忽略（进度不保证送达）。</summary>
    internal void Report(string json)
    {
        using var doc = JsonDocument.Parse(json);
        var o = doc.RootElement;
        if (!o.TryGetProperty("progress", out var p) || p.ValueKind != JsonValueKind.Number) return;
        progress.Report(new CallProgress(
            o.TryGetProperty("callId", out var id) ? id.GetString() ?? string.Empty : string.Empty,
            p.GetDouble(),
            o.TryGetProperty("total", out var t) && t.ValueKind == JsonValueKind.Number ? t.GetDouble() : null,
            o.TryGetProperty("message", out var m) && m.ValueKind == JsonValueKind.String ? m.GetString() : null));
    }
}
