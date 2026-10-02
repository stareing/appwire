using AppMcp.Native;

namespace AppMcp.Internal;

/// <summary>一次导航的原始 AmNavigate*，保证恰好完成一次。</summary>
internal sealed unsafe class PendingNavigate(nint navigate)
{
    private nint _navigate = navigate;

    private nint Take() => Interlocked.Exchange(ref _navigate, 0);

    public void Complete()
    {
        var n = Take();
        if (n != 0) NativeMethods.am_navigate_complete(n);
    }

    public void Fail(string message) => Finish(message, deny: false);

    public void Deny(string message) => Finish(message, deny: true);

    private void Finish(string message, bool deny)
    {
        var n = Take();
        if (n == 0) return;
        using var strings = new Utf8Strings();
        var msg = strings.AddPtr(message);
        if (deny) NativeMethods.am_navigate_deny(n, msg);
        else NativeMethods.am_navigate_fail(n, msg);
    }

    public void FailUserAction(string message, string? reason, string? uri)
    {
        var n = Take();
        if (n == 0) return;
        using var strings = new Utf8Strings();
        NativeMethods.am_navigate_fail_user_action(n, strings.AddPtr(message), strings.AddPtr(reason), strings.AddPtr(uri));
    }

    internal static void FailRaw(nint navigate, string message) => new PendingNavigate(navigate).Fail(message);
}

/// <summary>导航回调：在分发线程上读出请求，投递到调度器（UI 线程）执行 handler。</summary>
internal sealed class NavigationInvoker(Func<NavigationRequest, Task> handler, SynchronizationContext? dispatcher)
{
    public void Invoke(nint navigate)
    {
        // 指针在 navigate 被消费前有效：先在分发线程上复制。
        var page = NativeMethods.PtrToString(NativeMethods.am_navigate_page(navigate)) ?? string.Empty;
        var paramsJson = NativeMethods.PtrToString(NativeMethods.am_navigate_params_json(navigate));
        var pending = new PendingNavigate(navigate);
        Dispatch.Run(dispatcher, () => RunAsync(pending, new NavigationRequest(page, paramsJson)));
    }

    private async Task RunAsync(PendingNavigate pending, NavigationRequest request)
    {
        try
        {
            await handler(request).ConfigureAwait(false);
            pending.Complete();
        }
        catch (NavigationDeniedException e)
        {
            pending.Deny(e.Message);
        }
        catch (UserActionRequiredException e)
        {
            pending.FailUserAction(e.Message, e.Reason, e.Uri);
        }
        catch (Exception e)
        {
            pending.Fail(e.Message);
        }
    }
}
