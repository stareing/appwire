// 按名寻址示例（spec/naming.md，app_mcp.h v17）：在系统名字服务登记、不主动连接 Hub，由 Hub 拨号时接受通道；
// 通道关闭后若本进程由激活启动（命令行带 --app-mcp-activation）就退出，进程交还系统。
//
//   dotnet build samples/Named
//   app-mcp-host app install --app-id named-dotnet --exec <输出目录>/AppMcp.Samples.Named(.exe)
//       # Linux 写 D-Bus 激活文件；Windows 写 %LOCALAPPDATA%\app-mcp\apps\named-dotnet.json
//   app-mcp-host serve --name-service      # Hub 按名发现，调用时拨号（未运行则由系统激活）
//
// 环境变量：APP_MCP_APP_ID（默认 named-dotnet）；APP_MCP_NAME_INSTANCE（可选，登记实例名）；
// APP_MCP_EVENT_LOG（可选，追加 "start <pid>" / "exit <pid>" 行，测试用来核对激活次数与退出）。
// 工具：echo（原样返回参数）、pid（返回进程号）。

using AppMcp;

Note("start");
var idleExit = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
var instance = Environment.GetEnvironmentVariable("APP_MCP_NAME_INSTANCE");

await using (var client = AppMcpClient.Create(new AppMcpClientOptions
{
    AppId = Environment.GetEnvironmentVariable("APP_MCP_APP_ID") is { Length: > 0 } id ? id : "named-dotnet",
    AppName = "按名寻址示例（C#）",
    RegisterName = true,
    NameInstance = string.IsNullOrEmpty(instance) ? null : instance,
    Lifecycle = new LifecycleOptions { Mode = LifecycleMode.OnDemand, Residency = Residency.ExitWhenIdle },
    Dispatcher = null, // 控制台程序：handler 在线程池执行
}))
{
    client.Log += (_, e) => Console.Error.WriteLine($"[named_dotnet] {e.Level} {e.Message}");
    // 由激活启动：通道关闭后收到 IdleExit 即退出；用户直接运行时不会收到，一直等待（Ctrl+C 结束进程）。
    client.IdleExit += (_, _) => idleExit.TrySetResult();
    using var echo = client.RegisterTool("echo", "原样返回参数",
        (args, _) => Task.FromResult<object?>(new { echo = args }));
    using var pid = client.RegisterTool("pid", "返回进程号",
        (_, _) => Task.FromResult<object?>(new { pid = Environment.ProcessId }));
    client.Start();
    await idleExit.Task;
}
Note("exit");

static void Note(string evt)
{
    if (Environment.GetEnvironmentVariable("APP_MCP_EVENT_LOG") is { Length: > 0 } path)
    {
        File.AppendAllText(path, $"{evt} {Environment.ProcessId}\n");
    }
}
