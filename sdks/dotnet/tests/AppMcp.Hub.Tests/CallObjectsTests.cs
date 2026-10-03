using System.Text.Json;

namespace AppMcp.Hub.Tests;

/// <summary>调用对象（第 16 项 P5，spec/hub-api.md 3.6「调用对象」）：HubStatusInfo.Calls、内置工具 apps.calls / apps.cancel。</summary>
public class CallObjectsTests
{
    private static readonly TimeSpan Wait = TimeSpan.FromSeconds(10);

    private static async Task<T> WaitFor<T>(Func<T?> probe, string what) where T : class
    {
        var deadline = DateTime.UtcNow + Wait;
        for (;;)
        {
            if (probe() is { } v) return v;
            if (DateTime.UtcNow > deadline) throw new TimeoutException($"等待超时：{what}");
            await Task.Delay(20);
        }
    }

    [Fact]
    public void BuiltinsListed()
    {
        using var hub = AppMcpHub.Start(new HubOptions { DisableIpc = true, DisableListen = true, Dispatcher = null });
        var names = hub.ListTools().Select(t => t.Name).ToArray();
        Assert.Contains("apps.calls", names);
        Assert.Contains("apps.cancel", names);
        Assert.Empty(hub.Status().Calls!);
    }

    /// <summary>慢调用进行中：Status().Calls 含该调用（Running、进度）；apps.calls 只见自己的；apps.cancel 后发起方得到 CANCELLED。</summary>
    [Fact]
    public async Task InflightCallVisibleAndCancellable()
    {
        await using var hub = AppMcpHub.Start(new HubOptions { Listen = "127.0.0.1:0", DisableIpc = true, Dispatcher = null });
        await using var app = AppMcp.AppMcpClient.Create(new AppMcp.AppMcpClientOptions
        {
            AppId = "jobs",
            AppName = "作业",
            HostUrl = $"ws://{hub.ListenAddress}/app",
            Dispatcher = null,
        });
        using var tool = app.RegisterTool("job.run", "慢作业", async (_, ctx) =>
        {
            ctx.Progress(1, 4, "第一步");
            await Task.Delay(Timeout.Infinite, ctx.CancellationToken);
            return (object?)new { ok = true };
        });
        app.Start();
        await WaitFor(() => hub.ListTools(new ToolFilter { Apps = ["jobs"], OnlyAvailable = true, IncludeBuiltin = false }).Count == 1 ? "" : null,
            "工具同步");

        var slow = hub.CallAsync(new CallRequest("jobs.job.run") { CallId = "slow-1", Session = "s1" });
        var status = await WaitFor(
            () => hub.Status().Calls?.FirstOrDefault(c => c.CallId == "slow-1" && c.State == CallState.Running && c.Progress is not null),
            "slow-1 running");
        Assert.Equal(("jobs.job.run", "api:s1", "api"), (status.Name, status.Caller, status.Subject));
        Assert.False(string.IsNullOrEmpty(status.InstanceId));
        Assert.Equal(new CallStatusProgress(1, 4, "第一步"), status.Progress);

        var own = await hub.CallAsync(new CallRequest("apps.calls") { Session = "s1" });
        var calls = own.Data!.Value.GetProperty("calls");
        Assert.Equal("slow-1", Assert.Single(calls.EnumerateArray()).GetProperty("callId").GetString());
        Assert.False(calls[0].TryGetProperty("caller", out _));
        var other = await hub.CallAsync(new CallRequest("apps.calls") { Session = "s2" });
        Assert.Equal(0, other.Data!.Value.GetProperty("calls").GetArrayLength());
        var foreign = await hub.CallAsync(new CallRequest("apps.cancel", new { callId = "slow-1" }) { Session = "s2" });
        Assert.Equal("TOOL_NOT_FOUND", foreign.Error?.Kind);

        var cancelled = await hub.CallAsync(new CallRequest("apps.cancel", new { callId = "slow-1" }) { Session = "s1" });
        Assert.True(cancelled.IsSuccess, cancelled.Json.GetRawText());
        Assert.True(cancelled.Data!.Value.GetProperty("cancelled").GetBoolean());
        var outcome = await slow.WaitAsync(Wait);
        Assert.Equal("CANCELLED", outcome.Error?.Kind);
        await WaitFor(() => hub.Status().Calls!.Any(c => c.CallId == "slow-1") ? null : "", "slow-1 释放");
        app.Stop();
    }
}
