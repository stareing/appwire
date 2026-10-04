using System.Text.Json;

namespace AppMcp.Hub.Tests;

/// <summary>
/// 标准意图（第 16 项 N4，spec/intents.md）：真实 App 经 C ABI v21 声明 implements → HubToolInfo.Implements 与 Agent 会话 apps.intents 列出；
/// SetIntentDefaults 后默认工具排首位；不合法的默认表被拒、旧值保留且原因记入 Intents() / HubStatusInfo.Intents。
/// </summary>
public class IntentsTests
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

    /// <summary>apps.intents 中 link.open 的实现者（工具全名, 是否默认），按 Hub 给出的顺序。</summary>
    private static async Task<(string Tool, bool Default)[]> Implementations(AppMcpHub hub)
    {
        var outcome = await hub.CallAsync(new CallRequest("apps.intents", new { intent = "link.open" }) { Session = "s1" });
        Assert.True(outcome.IsSuccess, outcome.Json.GetRawText());
        var entry = Assert.Single(outcome.Data!.Value.GetProperty("intents").EnumerateArray());
        Assert.Equal("link.open@1", entry.GetProperty("intent").GetString());
        return entry.GetProperty("implementations").EnumerateArray()
            .Select(i => (i.GetProperty("tool").GetString()!,
                i.TryGetProperty("default", out var d) && d.ValueKind == JsonValueKind.True))
            .ToArray();
    }

    [Fact]
    public void BuiltinListedAndDefaultsEmpty()
    {
        using var hub = AppMcpHub.Start(new HubOptions { DisableIpc = true, DisableListen = true, Dispatcher = null });
        Assert.Contains("apps.intents", hub.ListTools().Select(t => t.Name));
        Assert.Empty(hub.Intents().Defaults);
        Assert.Null(hub.Intents().LastError);
        Assert.Empty(hub.Status().Intents!.Defaults);
    }

    [Fact]
    public async Task AppImplementsListedAndDefaultsOrderThem()
    {
        await using var hub = AppMcpHub.Start(new HubOptions { Listen = "127.0.0.1:0", DisableIpc = true, Dispatcher = null });
        await using var app = AppMcp.AppMcpClient.Create(new AppMcp.AppMcpClientOptions
        {
            AppId = "browser",
            AppName = "浏览器",
            HostUrl = $"ws://{hub.ListenAddress}/app",
            Dispatcher = null,
        });
        var options = new AppMcp.ToolOptions
        {
            InputSchemaJson = """{"type":"object","properties":{"url":{"type":"string"}},"required":["url"]}""",
            Implements = ["link.open@1"],
        };
        using var open = app.RegisterTool("open", "打开链接", (_, _) => Task.FromResult<object?>(null), options);
        using var tab = app.RegisterTool("tab", "在新标签页打开", (_, _) => Task.FromResult<object?>(null), options);
        app.Start();
        var tool = await WaitFor(() => hub.ListTools().FirstOrDefault(t => t.Name == "browser.open"), "App 工具");
        Assert.Equal(["link.open@1"], tool.Implements);

        // 无默认：按工具全名排序。
        Assert.Equal([("browser.open", false), ("browser.tab", false)], await Implementations(hub));

        // 设默认：默认工具排首位并标 default。
        hub.SetIntentDefaults(new Dictionary<string, string> { ["link.open"] = "browser.tab" });
        Assert.Equal([("browser.tab", true), ("browser.open", false)], await Implementations(hub));
        Assert.Equal("browser.tab", hub.Intents().Defaults["link.open"]);

        // 不合法的默认表：InvalidConfig，旧值保留，原因记入 Intents() 与 Status().Intents。
        var ex = Assert.Throws<HubException>(() =>
            hub.SetIntentDefaults(new Dictionary<string, string> { ["link"] = "browser.open" }));
        Assert.Equal(HubStatus.InvalidConfig, ex.Status);
        var intents = hub.Intents();
        Assert.Equal(new Dictionary<string, string> { ["link.open"] = "browser.tab" }, intents.Defaults);
        Assert.False(string.IsNullOrEmpty(intents.LastError));
        Assert.Equal(intents.LastError, hub.Status().Intents!.LastError);
        Assert.Equal(("browser.tab", true), (await Implementations(hub))[0]);

        // 成功替换后清除 LastError；空表清空。
        hub.SetIntentDefaults(new Dictionary<string, string>());
        Assert.Empty(hub.Intents().Defaults);
        Assert.Null(hub.Intents().LastError);
    }
}
