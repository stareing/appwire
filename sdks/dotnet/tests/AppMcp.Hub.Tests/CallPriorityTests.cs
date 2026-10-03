using System.Collections.Concurrent;
using System.Text.Json;
using System.Text.Json.Nodes;

namespace AppMcp.Hub.Tests;

/// <summary>调用优先级（第 16 项 P6，spec/hub-api.md 3.15）：CallRequest.Priority 写入请求 JSON，经 Hub 到达 App 的调用队列。</summary>
public class CallPriorityTests
{
    private static readonly TimeSpan Wait = TimeSpan.FromSeconds(10);

    private static JsonObject RequestJson(CallRequest req) => JsonNode.Parse(req.ToJson(new JsonSerializerOptions()))!.AsObject();

    [Fact]
    public void PrioritySerializes()
    {
        Assert.Equal("interactive", (string?)RequestJson(new CallRequest("a.b") { Priority = CallPriority.Interactive })["priority"]);
        Assert.Equal("normal", (string?)RequestJson(new CallRequest("a.b") { Priority = CallPriority.Normal })["priority"]);
        Assert.Equal("background", (string?)RequestJson(new CallRequest("a.b") { Priority = CallPriority.Background })["priority"]);
        Assert.False(RequestJson(new CallRequest("a.b")).ContainsKey("priority"));
    }

    /// <summary>App 并发 1：慢调用执行中先后到达后台、交互调用，交互调用先开始执行。</summary>
    [Fact]
    public async Task PriorityReachesAppQueue()
    {
        await using var hub = AppMcpHub.Start(new HubOptions { Listen = "127.0.0.1:0", DisableIpc = true, Dispatcher = null });
        await using var app = AppMcp.AppMcpClient.Create(new AppMcp.AppMcpClientOptions
        {
            AppId = "shop",
            AppName = "商城",
            HostUrl = $"ws://{hub.ListenAddress}/app",
            Dispatcher = null,
            MaxConcurrentCalls = 1,
        });
        var started = new ConcurrentQueue<string>();
        using var tool = app.RegisterTool("job.run", "执行", async (args, _) =>
        {
            if (args.TryGetProperty("tag", out var tag)) started.Enqueue(tag.GetString()!);
            if (args.TryGetProperty("delayMs", out var ms)) await Task.Delay(ms.GetInt32());
            return (object?)new { ok = true };
        });
        app.Start();
        var deadline = DateTime.UtcNow + Wait;
        while (hub.ListTools(new ToolFilter { Apps = ["shop"], OnlyAvailable = true, IncludeBuiltin = false }).Count != 1)
        {
            if (DateTime.UtcNow > deadline) throw new TimeoutException("工具未同步");
            await Task.Delay(20);
        }
        // 预热：首次调用的总览附带等不计入排队顺序
        Assert.True((await hub.CallAsync(new CallRequest("shop.job.run"))).IsSuccess);

        Task<CallOutcome> Call(string tag, CallPriority priority, int delayMs) =>
            hub.CallAsync(new CallRequest("shop.job.run", new { tag, delayMs }) { Priority = priority });
        var slow = Call("slow", CallPriority.Normal, 600);
        await Task.Delay(200);
        var background = Call("background", CallPriority.Background, 0);
        await Task.Delay(50);
        var interactive = Call("interactive", CallPriority.Interactive, 0);
        foreach (var o in await Task.WhenAll(slow, background, interactive).WaitAsync(Wait))
        {
            Assert.True(o.IsSuccess, o.Json.GetRawText());
        }
        Assert.Equal(["slow", "interactive", "background"], started.ToArray());
        app.Stop();
    }
}
