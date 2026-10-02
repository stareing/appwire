using System.Windows;
using System.Windows.Threading;

namespace AppMcp.Wpf.Tests.Support;

/// <summary>
/// 测试用 WPF 应用：每个进程只能有一个 <see cref="Application"/>，在专用 STA 线程上运行，测试体经其 Dispatcher 执行。
/// </summary>
internal static class WpfTestApp
{
    private static readonly Lazy<Dispatcher> Ui = new(Start);

    private static Dispatcher Start()
    {
        var ready = new TaskCompletionSource<Dispatcher>();
        var thread = new Thread(() =>
        {
            var app = new Application { ShutdownMode = ShutdownMode.OnExplicitShutdown };
            ready.SetResult(app.Dispatcher);
            app.Run();
        });
        thread.SetApartmentState(ApartmentState.STA);
        thread.IsBackground = true;
        thread.Start();
        return ready.Task.GetAwaiter().GetResult();
    }

    /// <summary>在 UI 线程上执行测试体（最长 30 秒）。</summary>
    public static async Task Run(Func<Task> body)
    {
        var run = Ui.Value.InvokeAsync(body).Task.Unwrap();
        if (await Task.WhenAny(run, Task.Delay(TimeSpan.FromSeconds(30))) != run) throw new TimeoutException("WPF 测试体超时");
        await run;
    }

    /// <summary>显示窗口并等到布局完成。</summary>
    public static async Task<Window> Show(Window window)
    {
        window.Show();
        await window.Dispatcher.InvokeAsync(static () => { }, DispatcherPriority.ApplicationIdle);
        return window;
    }
}
