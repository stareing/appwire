using System.ComponentModel;
using System.IO;
using System.Windows;
using AppMcp.Activation;

namespace AppMcp.Samples.Wpf;

/// <summary>
/// 在 UI 线程上创建 <see cref="AppMcpClient"/>：它会捕获 WPF 的 DispatcherSynchronizationContext，
/// 之后所有 handler 与事件都在 UI 线程上执行，可以直接读写控件。
/// </summary>
public partial class MainWindow : Window
{
    private static readonly string TokenFile = Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "AppMcpWpfSample", "token.txt");

    private AppMcpClient? _client;
    private int _counter;
    private ResourceRegistration? _counterResource;
    private bool _userInteracted = !App.LaunchedByWake;

    public MainWindow()
    {
        InitializeComponent();
    }

    private void OnLoaded(object sender, RoutedEventArgs e)
    {
        try
        {
            _client = AppMcpClient.Create(new AppMcpClientOptions
            {
                AppId = "wpf-sample",
                AppName = "WPF 示例",
                InstanceTitle = Title,
                Token = LoadToken(),
                Overview = new AppOverview(
                    "WPF 计数器示例：读取和修改计数、写备注",
                    "## 能力\n- counter.get / counter.increment：读取、增加计数\n- note.set：修改备注",
                    "zh-CN"),
                // Dispatcher 未设置：自动捕获当前（UI 线程）的 SynchronizationContext。
                // 生命周期：空闲 60 秒后休眠（断开连接、释放运行时线程）；Host 需要时通过 URL scheme / AUMID 唤醒。
                Lifecycle = new LifecycleOptions
                {
                    Mode = LifecycleMode.Idle,
                    Residency = Residency.ExitWhenIdle, // 仅由唤醒冷启动的进程会收到 IdleExit
                    Wake = WakeDescriptorFactory.ForWindows(App.UriScheme),
                },
            });
        }
        catch (AppMcpException ex)
        {
            StatusText.Text = $"创建客户端失败：{ex.Status} {ex.Message}";
            return;
        }

        _client.StateChanged += (_, args) => StatusText.Text = args.State.Status switch
        {
            ClientStatus.Connected => "已连接 Host",
            ClientStatus.PendingPairing => "等待在 Host 中确认配对…",
            ClientStatus.Backoff => $"未连接，{args.State.RetryIn?.TotalSeconds:0} 秒后重试",
            ClientStatus.Rejected => $"配对被拒绝：{args.State.Reason}",
            var s => s.ToString(),
        };
        _client.Paired += (_, args) => SaveToken(args.Token);

        // exit-when-idle：本进程由 Host 唤醒冷启动、任务完成并休眠后触发（在 UI 线程上）。
        // 用户已经在使用窗口时（如激活过窗口）可以选择不退出。
        _client.IdleExit += (_, _) =>
        {
            if (!_userInteracted) Application.Current.Shutdown();
        };

        // 冷启动唤醒：启动参数里的唤醒令牌交给客户端；之后第二实例转交的参数由 SingleInstance 转给客户端。
        _client.HandleWake(App.StartupArgs);
        App.Single?.AttachClient(_client);
        if (App.Single is { } single)
        {
            single.Activated += (_, args) =>
            {
                // 非唤醒的普通激活（如用户再次双击图标）：把窗口带到前台。
                if (!args.WakeHandled) Activate();
            };
        }

        _client.RegisterTool<Empty, CounterState>(
            "counter.get",
            "读取当前计数",
            (_, _) => Task.FromResult(new CounterState(_counter)),
            new ToolOptions { Risk = ToolRisk.Read, Activation = ToolActivation.Background });

        _client.RegisterTool<IncrementInput, CounterState>(
            "counter.increment",
            "把计数增加 by（默认 1）",
            async (input, ctx) =>
            {
                // 在 UI 线程上执行：可以直接修改控件。
                await Task.Delay(200, ctx.CancellationToken); // 模拟动画
                SetCounter(_counter + (input.By ?? 1));
                ctx.AddStateHint("counter");
                return new CounterState(_counter);
            },
            new ToolOptions { Risk = ToolRisk.Write, Activation = ToolActivation.Foreground, Title = "增加计数" });

        _client.RegisterTool<NoteInput, NoteInput>(
            "note.set",
            "修改备注内容",
            (input, _) =>
            {
                if (input.Text.Length > 1000)
                {
                    throw new ToolCallException(ToolErrorKind.InvalidInput, "备注不能超过 1000 个字符");
                }
                NoteBox.Text = input.Text;
                return Task.FromResult(input);
            });

        _counterResource = _client.RegisterResource(
            "counter",
            "当前计数",
            _ => Task.FromResult(new CounterState(_counter)));

        _client.Start();
    }

    private void SetCounter(int value)
    {
        _counter = value;
        CounterText.Text = value.ToString();
        try
        {
            _counterResource?.NotifyChanged();
        }
        catch (AppMcpException)
        {
            // 客户端已停止时忽略。
        }
    }

    private void OnIncrementClick(object sender, RoutedEventArgs e)
    {
        _userInteracted = true;
        SetCounter(_counter + 1);
    }

    private void OnActivated(object? sender, EventArgs e) => UpdateVisibility(focused: true);

    private void OnDeactivated(object? sender, EventArgs e) => UpdateVisibility(focused: false);

    private void OnWindowStateChanged(object? sender, EventArgs e) => UpdateVisibility(IsActive);

    private void UpdateVisibility(bool focused)
    {
        if (_client is null) return;
        var visibility = WindowState == WindowState.Minimized ? AppVisibility.Hidden : AppVisibility.Visible;
        try
        {
            _client.SetVisibility(visibility, focused);
        }
        catch (AppMcpException)
        {
        }
    }

    private async void OnClosing(object? sender, CancelEventArgs e)
    {
        App.Single?.AttachClient(null);
        var client = _client;
        _client = null;
        if (client is not null)
        {
            // DisposeAsync 在线程池上释放，避免阻塞 UI 线程。
            await client.DisposeAsync();
        }
    }

    private static string? LoadToken()
    {
        try
        {
            return File.Exists(TokenFile) ? File.ReadAllText(TokenFile).Trim() : null;
        }
        catch (IOException)
        {
            return null;
        }
    }

    private static void SaveToken(string token)
    {
        try
        {
            Directory.CreateDirectory(Path.GetDirectoryName(TokenFile)!);
            File.WriteAllText(TokenFile, token);
        }
        catch (IOException)
        {
        }
    }
}

public sealed record Empty;

public sealed record CounterState(int Value);

public sealed record IncrementInput(int? By);

public sealed record NoteInput(string Text);
