// 控制台示例：注册 greet({name}) 工具与 app.info 资源，连接 Host，Ctrl+C 退出。
//
// 用法：dotnet run --project samples/Console -- [ws://127.0.0.1:7717/app]
// 也可用环境变量 APP_MCP_HOST_URL；原生库路径可用 APP_MCP_NATIVE_PATH 指定。

using AppMcp;

var hostUrl = args.Length > 0 ? args[0] : Environment.GetEnvironmentVariable("APP_MCP_HOST_URL");

// 控制台程序没有 SynchronizationContext，handler 在线程池上执行。
await using var client = AppMcpClient.Create(new AppMcpClientOptions
{
    AppId = "hello-dotnet",
    AppName = "Hello .NET",
    HostUrl = hostUrl,
    Overview = new AppOverview("演示用的 .NET App：向人问好", "## 能力\n- greet：问好", "zh-CN"),
});

client.StateChanged += (_, e) => Console.Error.WriteLine($"[hello_dotnet] state={e.State.Status} {e.State.Reason}");
client.Paired += (_, e) => Console.Error.WriteLine($"[hello_dotnet] paired, token={e.Token}");

using var greet = client.RegisterTool<GreetInput, GreetOutput>(
    "greet",
    "向指定的人问好",
    async (input, ctx) =>
    {
        if (string.IsNullOrEmpty(input.Name))
        {
            throw new ToolCallException(ToolErrorKind.InvalidInput, "缺少 name 参数");
        }
        await Task.Delay(10, ctx.CancellationToken);
        return new GreetOutput($"Hello, {input.Name}!");
    },
    new ToolOptions { Risk = ToolRisk.Read });

using var info = client.RegisterResource(
    "app.info",
    "App 基本信息",
    _ => Task.FromResult(new AppInfo("hello_dotnet", "c#")));

client.Start();
Console.Error.WriteLine($"[hello_dotnet] app_mcp {AppMcpClient.Version} 已启动，Ctrl+C 退出");

var quit = new TaskCompletionSource();
Console.CancelKeyPress += (_, e) =>
{
    e.Cancel = true;
    quit.TrySetResult();
};
AppDomain.CurrentDomain.ProcessExit += (_, _) => quit.TrySetResult();
await quit.Task;
Console.Error.WriteLine("[hello_dotnet] 退出");

internal sealed record GreetInput(string Name);

internal sealed record GreetOutput(string Greeting);

internal sealed record AppInfo(string App, string Lang);
