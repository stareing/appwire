using System.Windows;
using AppMcp.Activation;

namespace AppMcp.Samples.Wpf;

/// <summary>
/// 单实例重定向（spec/lifecycle.md 第 5 节）：Host 唤醒未打包应用时会以
/// <c>"exe" "wpf-sample:app-mcp/wake?token=…"</c> 启动第二个进程；这里把参数转交给已运行的实例后立即退出，
/// 已运行的实例在 <see cref="MainWindow"/> 中通过 <see cref="SingleInstance.AttachClient"/> 调用 HandleWake。
/// </summary>
public partial class App : Application
{
    internal const string UriScheme = "wpf-sample";

    /// <summary>首实例的单实例监听（第二实例为 null，且已退出）。</summary>
    internal static SingleInstance? Single { get; private set; }

    /// <summary>本进程的启动参数（冷启动唤醒时含唤醒令牌）。</summary>
    internal static string[] StartupArgs { get; private set; } = [];

    /// <summary>本进程是否由 Host 唤醒冷启动（residency = exit-when-idle 时，只有这种进程才会收到 IdleExit）。</summary>
    internal static bool LaunchedByWake { get; private set; }

    protected override void OnStartup(StartupEventArgs e)
    {
        StartupArgs = e.Args;
        LaunchedByWake = AppMcpClient.IsWakeArgs(e.Args);

        Single = SingleInstance.Acquire("app-mcp-wpf-sample", e.Args);
        if (Single is null)
        {
            // 参数已转交给已运行的实例。
            Shutdown();
            return;
        }

        // 未打包应用：注册 URL scheme（HKCU，无需管理员权限）。打包（MSIX）应用改用 AUMID，不需要注册。
        var exe = Environment.ProcessPath;
        if (exe is not null && !ProtocolRegistration.IsRegistered(UriScheme, exe))
        {
            ProtocolRegistration.Register(UriScheme, exe, "App MCP WPF 示例");
        }

        base.OnStartup(e);
        new MainWindow().Show();
    }

    protected override void OnExit(ExitEventArgs e)
    {
        Single?.Dispose();
        base.OnExit(e);
    }
}
