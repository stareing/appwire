// Windows 唤醒端到端测试用的无窗口 App（WinExe，不弹控制台）。说明见 tests/windows/README.md。
//
//   AppMcpWakeApp.exe --register      注册 HKCU\Software\Classes\appmcp-wintest → 本程序
//   AppMcpWakeApp.exe --unregister    删除该键
//   AppMcpWakeApp.exe --quit          转交给已运行的实例，使其退出
//   AppMcpWakeApp.exe [激活参数...]    正常运行：单实例；idle 模式连接 Host（APP_MCP_HOST_URL，默认 ws://127.0.0.1:7791）
//
// 输出写入日志文件（APPMCP_WINTEST_LOG，默认程序目录下 wakeapp.log），供测试脚本判定。

using System.Diagnostics;
using AppMcp;
using AppMcp.Activation;

const string Scheme = "appmcp-wintest";
const string SingleKey = "appmcp-wintest";

var logPath = Environment.GetEnvironmentVariable("APPMCP_WINTEST_LOG") ?? Path.Combine(AppContext.BaseDirectory, "wakeapp.log");
var logLock = new object();
void Log(string message)
{
    var line = $"{DateTime.Now:HH:mm:ss.fff} [pid {Environment.ProcessId}] {message}{Environment.NewLine}";
    lock (logLock)
    {
        for (var i = 0; i < 20; i++)
        {
            try { File.AppendAllText(logPath, line); return; }
            catch (IOException) { Thread.Sleep(10); } // 另一个实例同时在写
        }
    }
}

var exe = Environment.ProcessPath ?? throw new InvalidOperationException("无法取得程序路径");

if (args is ["--register"])
{
    ProtocolRegistration.Register(Scheme, exe, "App MCP Windows 唤醒测试");
    Log($"REGISTERED {ProtocolRegistration.IsRegistered(Scheme, exe)} {ProtocolRegistration.KeyPath(Scheme)}");
    return 0;
}
if (args is ["--unregister"])
{
    ProtocolRegistration.Unregister(Scheme);
    Log($"UNREGISTERED {!ProtocolRegistration.IsRegistered(Scheme, exe)}");
    return 0;
}

Log($"START args=[{string.Join(" ", args)}] isWake={AppMcpClient.IsWakeArgs(args)}");

SingleInstance? single;
try
{
    single = SingleInstance.Acquire(SingleKey, args);
}
catch (TimeoutException e)
{
    Log($"FORWARD_FAILED {e.Message}");
    return 2;
}
if (single is null)
{
    Log("FORWARDED 已把参数转交给首实例，退出");
    return 0;
}
if (args is ["--quit"])
{
    Log("QUIT 没有运行中的实例");
    single.Dispose();
    return 0;
}

var hostUrl = Environment.GetEnvironmentVariable("APP_MCP_HOST_URL") ?? "ws://127.0.0.1:7791";
var launchedByWake = AppMcpClient.IsWakeArgs(args);
var started = Stopwatch.StartNew();
var quit = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);

await using var client = AppMcpClient.Create(new AppMcpClientOptions
{
    AppId = "wintest",
    AppName = "Windows 唤醒测试",
    HostUrl = hostUrl,
    Dispatcher = null,
    ConnectTimeout = TimeSpan.FromSeconds(5),
    Lifecycle = new LifecycleOptions
    {
        Mode = LifecycleMode.Idle,
        IdleTimeout = TimeSpan.FromSeconds(3),
        Residency = Residency.Keep,
        Wake = WakeDescriptorFactory.ForWindows(Scheme, background: true),
    },
});

client.StateChanged += (_, e) => Log($"STATE {e.State.Status} {e.State.Reason}");

using var echo = client.RegisterTool<EchoInput, EchoOutput>(
    "echo",
    "回显文本，并返回进程信息",
    (input, _) => Task.FromResult(new EchoOutput(input.Text ?? "", Environment.ProcessId, launchedByWake, (long)started.Elapsed.TotalMilliseconds)),
    new ToolOptions { Risk = ToolRisk.Read, Activation = ToolActivation.Headless });

var handled = client.HandleWake(args);
Log($"HANDLE_WAKE cold={handled}");
single.Activated += (_, e) =>
{
    Log($"ACTIVATED args=[{string.Join(" ", e.Args)}] wakeHandled={e.WakeHandled}");
    if (e.Args.Contains("--quit")) quit.TrySetResult();
};
single.AttachClient(client);
client.Start();
Log($"RUNNING host={hostUrl} toolsHash={client.ToolsHash}");

// 测试程序：最多运行 5 分钟，避免遗留进程。
var done = await Task.WhenAny(quit.Task, Task.Delay(TimeSpan.FromMinutes(5)));
Log(done == quit.Task ? "EXIT 收到 --quit" : "EXIT 超过最长运行时间");
single.Dispose();
return 0;

internal sealed record EchoInput(string? Text);

internal sealed record EchoOutput(string Text, int Pid, bool LaunchedByWake, long UptimeMs);
