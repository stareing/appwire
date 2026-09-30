using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using AppMcp.Native;

namespace AppMcp.Internal;

/// <summary>
/// 传给 C 接口的回调。user_data 为 <see cref="GCHandle"/>，由 <see cref="FreeGCHandle"/>（free_user_data）释放。
/// 回调在库的分发线程上执行，这里只做最少的工作（投递到调度器），并吞掉所有异常——异常不能跨越 FFI 边界。
/// </summary>
internal static unsafe class Callbacks
{
    internal static nint FreeGCHandlePtr => (nint)(delegate* unmanaged[Cdecl]<nint, void>)&FreeGCHandle;
    internal static nint ToolPtr => (nint)(delegate* unmanaged[Cdecl]<nint, nint, void>)&OnToolCall;
    internal static nint ReadPtr => (nint)(delegate* unmanaged[Cdecl]<nint, nint, void>)&OnRead;
    internal static nint CancelPtr => (nint)(delegate* unmanaged[Cdecl]<nint, int, void>)&OnCancel;
    internal static nint StatePtr => (nint)(delegate* unmanaged[Cdecl]<nint, int, ulong, nint, void>)&OnState;
    internal static nint PairedPtr => (nint)(delegate* unmanaged[Cdecl]<nint, nint, void>)&OnPaired;
    internal static nint LogPtr => (nint)(delegate* unmanaged[Cdecl]<nint, int, nint, void>)&OnLog;
    // 静态 UnmanagedCallersOnly 函数指针，不存在委托被 GC 回收的问题。
    internal static nint IdleExitPtr => (nint)(delegate* unmanaged[Cdecl]<nint, void>)&OnIdleExit;

    internal static nint Alloc(object target) => GCHandle.ToIntPtr(GCHandle.Alloc(target));

    private static T? Target<T>(nint userData) where T : class =>
        userData == 0 ? null : GCHandle.FromIntPtr(userData).Target as T;

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    private static void FreeGCHandle(nint userData)
    {
        try
        {
            if (userData != 0) GCHandle.FromIntPtr(userData).Free();
        }
        catch
        {
            // 忽略：不能让异常跨越 FFI 边界。
        }
    }

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    private static void OnToolCall(nint userData, nint call)
    {
        try
        {
            if (Target<ToolInvoker>(userData) is { } invoker)
            {
                invoker.Invoke(call);
                return;
            }
            PendingCall.FailRaw(call, "HANDLER_ERROR", "handler 已释放");
        }
        catch (Exception e)
        {
            PendingCall.FailRaw(call, "HANDLER_ERROR", e.Message);
        }
    }

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    private static void OnRead(nint userData, nint read)
    {
        try
        {
            if (Target<ResourceInvoker>(userData) is { } invoker)
            {
                invoker.Invoke(read);
                return;
            }
            PendingRead.FailRaw(read, "HANDLER_ERROR", "reader 已释放");
        }
        catch (Exception e)
        {
            PendingRead.FailRaw(read, "HANDLER_ERROR", e.Message);
        }
    }

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    private static void OnCancel(nint userData, int reason)
    {
        try
        {
            Target<PendingCall>(userData)?.OnCancelled((CancelReason)reason);
        }
        catch
        {
        }
    }

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    private static void OnState(nint userData, int status, ulong retryInMs, nint reason)
    {
        try
        {
            // 字符串归回调方所有（AM_API_VERSION 2）：先取出并释放。
            var reasonText = NativeMethods.TakeString(reason);
            var st = (ClientStatus)status;
            var state = new ClientState(
                st,
                st == ClientStatus.Backoff ? TimeSpan.FromMilliseconds(retryInMs) : null,
                reasonText);
            Target<ClientEventSink>(userData)?.OnState(state);
        }
        catch
        {
        }
    }

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    private static void OnPaired(nint userData, nint token)
    {
        try
        {
            var text = NativeMethods.TakeString(token) ?? string.Empty;
            Target<ClientEventSink>(userData)?.OnPaired(text);
        }
        catch
        {
        }
    }

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    private static void OnLog(nint userData, int level, nint message)
    {
        try
        {
            var text = NativeMethods.TakeString(message) ?? string.Empty;
            Target<ClientEventSink>(userData)?.OnLog((LogLevel)level, text);
        }
        catch
        {
        }
    }

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    private static void OnIdleExit(nint userData)
    {
        try
        {
            Target<ClientEventSink>(userData)?.OnIdleExit();
        }
        catch
        {
        }
    }
}

/// <summary>把工作投递到调度器（SynchronizationContext）或线程池。</summary>
internal static class Dispatch
{
    public static void Run(SynchronizationContext? dispatcher, Func<Task> work)
    {
        if (dispatcher is null)
        {
            _ = Task.Run(work);
        }
        else
        {
            dispatcher.Post(static s => _ = ((Func<Task>)s!)(), work);
        }
    }

    public static void Run(SynchronizationContext? dispatcher, Action work)
    {
        if (dispatcher is null)
        {
            ThreadPool.UnsafeQueueUserWorkItem(static a => a(), work, preferLocal: false);
        }
        else
        {
            dispatcher.Post(static s => ((Action)s!)(), work);
        }
    }
}

/// <summary>客户端事件的接收者。只弱引用客户端，避免 GCHandle 让未 Dispose 的客户端永远无法回收。</summary>
internal sealed class ClientEventSink(SynchronizationContext? dispatcher)
{
    private WeakReference<AppMcpClient>? _client;

    public void Attach(AppMcpClient client) => _client = new WeakReference<AppMcpClient>(client);

    private void Raise(Action<AppMcpClient> raise)
    {
        if (_client is null || !_client.TryGetTarget(out var client)) return;
        if (dispatcher is null)
        {
            // 未指定调度器：直接在分发线程上触发（订阅者必须尽快返回）。
            try { raise(client); } catch { }
            return;
        }
        dispatcher.Post(_ =>
        {
            if (_client.TryGetTarget(out var c)) raise(c);
        }, null);
    }

    public void OnState(ClientState state) => Raise(c => c.RaiseStateChanged(state));
    public void OnPaired(string token) => Raise(c => c.RaisePaired(token));
    public void OnLog(LogLevel level, string message) => Raise(c => c.RaiseLog(level, message));
    public void OnIdleExit() => Raise(c => c.RaiseIdleExit());
}
