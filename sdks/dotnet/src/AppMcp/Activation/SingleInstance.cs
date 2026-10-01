using System.IO.Pipes;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using AppMcp.Internal;

namespace AppMcp.Activation;

/// <summary>另一个实例转交过来的激活参数。</summary>
public sealed class ActivationEventArgs(IReadOnlyList<string> args, bool wakeHandled) : EventArgs
{
    /// <summary>第二实例的命令行参数（不含可执行文件路径）。</summary>
    public IReadOnlyList<string> Args { get; } = args;

    /// <summary>参数中含 app-mcp 唤醒且已交给 <see cref="AppMcpClient.HandleWake(string)"/>。</summary>
    public bool WakeHandled { get; } = wakeHandled;
}

/// <summary>
/// 单实例重定向（spec/lifecycle.md 第 5 节 Windows 行）：<c>Mutex</c> 判定首实例，命名管道把第二实例的激活参数
/// （协议激活 URI、<c>app-mcp-wake:&lt;token&gt;</c>）转交给已运行的实例，由它调用 <see cref="AppMcpClient.HandleWake(string)"/>。
/// </summary>
/// <remarks>
/// <para>用法（WPF <c>App.OnStartup</c>）：</para>
/// <code>
/// var single = SingleInstance.Acquire("my-app", e.Args);
/// if (single is null) { Shutdown(); return; }   // 参数已转交给已运行的实例
/// single.AttachClient(client);                  // 之后收到的唤醒参数自动交给客户端
/// </code>
/// <para>管道使用 <see cref="PipeOptions.CurrentUserOnly"/>，只接受同一用户的连接；Windows 与 Unix（Unix 域套接字）都可用。</para>
/// </remarks>
public sealed class SingleInstance : IDisposable
{
    private readonly Mutex _mutex;
    private readonly string _pipeName;
    private readonly SynchronizationContext? _dispatcher;
    private readonly CancellationTokenSource _cts = new();
    private readonly Task _loop;
    private volatile AppMcpClient? _client;
    private int _disposed;

    private SingleInstance(Mutex mutex, string pipeName, SynchronizationContext? dispatcher)
    {
        _mutex = mutex;
        _pipeName = pipeName;
        _dispatcher = dispatcher;
        _loop = Task.Run(() => ListenAsync(_cts.Token));
    }

    /// <summary>收到另一个实例转交的参数（在创建时的 <see cref="SynchronizationContext"/> 上触发；没有时在线程池上）。</summary>
    public event EventHandler<ActivationEventArgs>? Activated;

    /// <summary>
    /// 尝试成为首实例。成功时返回实例（开始监听管道）；已有实例运行时把 <paramref name="args"/> 转交给它并返回 null，
    /// 调用方应随即退出。转交失败（首实例无响应）时抛出 <see cref="TimeoutException"/>。
    /// </summary>
    /// <param name="key">应用唯一键（如 app id）。</param>
    /// <param name="args">本进程的命令行参数（不含可执行文件路径）。</param>
    /// <param name="forwardTimeout">转交超时，默认 5 秒（首实例可能刚拿到 Mutex、尚未开始监听）。</param>
    public static SingleInstance? Acquire(string key, IReadOnlyList<string> args, TimeSpan? forwardTimeout = null)
    {
        ArgumentException.ThrowIfNullOrEmpty(key);
        ArgumentNullException.ThrowIfNull(args);
        var (mutexName, pipeName) = Names(key);
        var mutex = new Mutex(initiallyOwned: true, mutexName, out var createdNew);
        if (createdNew) return new SingleInstance(mutex, pipeName, SynchronizationContext.Current);
        mutex.Dispose();
        ForwardAsync(pipeName, args, forwardTimeout ?? TimeSpan.FromSeconds(5)).GetAwaiter().GetResult();
        return null;
    }

    /// <summary>
    /// 桌面平台默认生命周期（spec/lifecycle.md 第 13 节 B1"平台默认"）。本实例已在接收第二实例转交的激活参数，
    /// 配上 <paramref name="wake"/>（通常来自 <see cref="WakeDescriptorFactory.ForWindows"/>）后休眠中仍可被唤醒：
    /// <list type="bullet">
    /// <item><c>wake.Kind == None</c>：<see cref="LifecycleMode.Persistent"/>——Host 只能按清单 <c>launch</c> 冷启动，休眠后可能不可达。</item>
    /// <item><c>wake.Background</c>（托盘 / 无窗口进程）：<see cref="LifecycleMode.OnDemand"/> + <see cref="LifecycleOptions.SleepOnBackground"/>。</item>
    /// <item>否则（窗口程序）：<see cref="LifecycleMode.Idle"/>，调用后 2 秒合并窗口。</item>
    /// </list>
    /// 个别字段用 <c>with</c> 覆盖，如 <c>single.DefaultLifecycle(wake) with { IdleTimeout = ... }</c>。
    /// </summary>
    public LifecycleOptions DefaultLifecycle(WakeDescriptor wake) => DesktopLifecycle(wake);

    /// <summary><see cref="DefaultLifecycle"/> 的规则（不依赖单实例监听，便于测试）。</summary>
    internal static LifecycleOptions DesktopLifecycle(WakeDescriptor wake)
    {
        ArgumentNullException.ThrowIfNull(wake);
        if (wake.Kind == WakeKind.None) return new LifecycleOptions();
        if (wake.Background) return new LifecycleOptions { Mode = LifecycleMode.OnDemand, SleepOnBackground = true, Wake = wake };
        return new LifecycleOptions { Mode = LifecycleMode.Idle, Wake = wake };
    }

    /// <summary>把之后收到的激活参数交给 <paramref name="client"/>.HandleWake（在管道线程上调用，HandleWake 线程安全）。</summary>
    public void AttachClient(AppMcpClient? client) => _client = client;

    /// <summary>由 key 得到 Mutex 名与管道名（管道名控制在 Unix 域套接字路径长度内）。</summary>
    internal static (string Mutex, string Pipe) Names(string key)
    {
        var raw = $"{key}|{Environment.UserName}";
        var hash = Convert.ToHexString(SHA256.HashData(Encoding.UTF8.GetBytes(raw)))[..16].ToLowerInvariant();
        var mutex = OperatingSystem.IsWindows() ? $@"Local\app-mcp-{hash}" : $"app-mcp-{hash}";
        return (mutex, $"app-mcp-{hash}");
    }

    internal static async Task ForwardAsync(string pipeName, IReadOnlyList<string> args, TimeSpan timeout)
    {
        var payload = JsonSerializer.SerializeToUtf8Bytes(args.ToArray());
        using var cts = new CancellationTokenSource(timeout);
        while (true)
        {
            try
            {
                await using var pipe = new NamedPipeClientStream(".", pipeName, PipeDirection.Out, PipeOptions.CurrentUserOnly | PipeOptions.Asynchronous);
                await pipe.ConnectAsync(cts.Token).ConfigureAwait(false);
                await pipe.WriteAsync(payload, cts.Token).ConfigureAwait(false);
                await pipe.FlushAsync(cts.Token).ConfigureAwait(false);
                return;
            }
            catch (OperationCanceledException)
            {
                throw new TimeoutException("已运行的实例没有响应单实例管道");
            }
            catch (IOException)
            {
                // 首实例正在重建监听端：稍后重试。
                await Task.Delay(50, cts.Token).ConfigureAwait(false);
            }
        }
    }

    private async Task ListenAsync(CancellationToken ct)
    {
        while (!ct.IsCancellationRequested)
        {
            try
            {
                await using var server = new NamedPipeServerStream(_pipeName, PipeDirection.In, NamedPipeServerStream.MaxAllowedServerInstances,
                    PipeTransmissionMode.Byte, PipeOptions.CurrentUserOnly | PipeOptions.Asynchronous);
                await server.WaitForConnectionAsync(ct).ConfigureAwait(false);
                using var buffer = new MemoryStream();
                using (var read = new CancellationTokenSource(TimeSpan.FromSeconds(5)))
                using (var linked = CancellationTokenSource.CreateLinkedTokenSource(ct, read.Token))
                {
                    await server.CopyToAsync(buffer, linked.Token).ConfigureAwait(false);
                }
                string[] args;
                try { args = JsonSerializer.Deserialize<string[]>(buffer.ToArray()) ?? []; }
                catch (JsonException) { continue; }
                Deliver(args);
            }
            catch (OperationCanceledException) when (ct.IsCancellationRequested)
            {
                return;
            }
            catch (Exception)
            {
                // 单个连接出错不影响后续监听。
                if (!ct.IsCancellationRequested) await Task.Delay(50, CancellationToken.None).ConfigureAwait(false);
            }
        }
    }

    private void Deliver(string[] args)
    {
        var handled = false;
        if (_client is { } client)
        {
            try { handled = client.HandleWake(args); }
            catch (Exception) { }
        }
        var e = new ActivationEventArgs(args, handled);
        Dispatch.Run(_dispatcher, () =>
        {
            try { Activated?.Invoke(this, e); } catch { }
        });
    }

    /// <summary>停止监听并释放 Mutex。</summary>
    public void Dispose()
    {
        if (Interlocked.Exchange(ref _disposed, 1) != 0) return;
        _cts.Cancel();
        try { _loop.Wait(TimeSpan.FromSeconds(2)); } catch { }
        // Mutex 有线程亲和性：不在创建线程上释放时 ReleaseMutex 会抛出，关闭句柄即可（进程退出时同样释放）。
        try { _mutex.ReleaseMutex(); } catch (ApplicationException) { }
        _mutex.Dispose();
        _cts.Dispose();
    }
}
