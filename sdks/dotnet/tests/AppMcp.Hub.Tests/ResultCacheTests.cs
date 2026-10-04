namespace AppMcp.Hub.Tests;

/// <summary>
/// 只读结果缓存（第 16 项 O3，spec/hub-api.md 3.20）：真实 App 经 C ABI v22 声明 cache → 第二次调用命中（CachedAgeMs、handler 不再执行）、
/// CacheBypass 照常调用、资源读取命中计入 Status().Cache、HubOptions.ResultCache.MaxEntries = 0 关闭缓存。
/// </summary>
public class ResultCacheTests
{
    private static readonly TimeSpan Wait = TimeSpan.FromSeconds(10);

    private static async Task WaitTool(AppMcpHub hub, string name)
    {
        var deadline = DateTime.UtcNow + Wait;
        while (!hub.ListTools().Any(t => t.Name == name))
        {
            if (DateTime.UtcNow > deadline) throw new TimeoutException($"等待超时：{name}");
            await Task.Delay(20);
        }
    }

    /// <summary>注册 quote.get（只读、cache 60 s shared）与资源 quotes（cache 30 s）的 App；runs 为 handler 执行次数。</summary>
    private sealed class QuoteApp : IAsyncDisposable
    {
        public int Runs;
        public int Reads;
        private readonly AppMcp.AppMcpClient _client;
        private readonly AppMcp.ToolRegistration _tool;
        private readonly AppMcp.ResourceRegistration _resource;

        public QuoteApp(AppMcpHub hub)
        {
            _client = AppMcp.AppMcpClient.Create(new AppMcp.AppMcpClientOptions
            {
                AppId = "quote",
                AppName = "报价",
                HostUrl = $"ws://{hub.ListenAddress}/app",
                Dispatcher = null,
            });
            var options = new AppMcp.ToolOptions
            {
                Risk = AppMcp.ToolRisk.Read,
                Cache = new AppMcp.CachePolicy(60_000, AppMcp.CacheScope.Shared),
            };
            _tool = _client.RegisterTool("get", "查询报价",
                (_, _) => Task.FromResult<object?>(new { n = Interlocked.Increment(ref Runs) }), options);
            _resource = _client.RegisterResource("quotes", "全部报价",
                _ => Task.FromResult<object?>(new { r = Interlocked.Increment(ref Reads) }),
                cache: new AppMcp.CachePolicy(30_000));
            _client.Start();
        }

        public async ValueTask DisposeAsync()
        {
            _tool.Dispose();
            _resource.Dispose();
            await _client.DisposeAsync();
        }
    }

    private static Task<CallOutcome> Quote(AppMcpHub hub, bool bypass = false) =>
        hub.CallAsync(new CallRequest("quote.get", new { symbol = "ACME" }) { CacheBypass = bypass });

    [Fact]
    public async Task HitBypassAndStatus()
    {
        await using var hub = AppMcpHub.Start(new HubOptions { Listen = "127.0.0.1:0", DisableIpc = true, Dispatcher = null });
        await using var app = new QuoteApp(hub);
        await WaitTool(hub, "quote.get");

        var first = await Quote(hub);
        Assert.True(first.IsSuccess, first.Json.GetRawText());
        Assert.Equal(1, first.Data!.Value.GetProperty("n").GetInt32());
        Assert.Null(first.CachedAgeMs);
        var second = await Quote(hub);
        Assert.Equal(1, second.Data!.Value.GetProperty("n").GetInt32());
        Assert.NotNull(second.CachedAgeMs);
        Assert.True(second.CachedAgeMs >= 0);
        Assert.False(second.Woke);
        Assert.Equal(1, app.Runs);

        var fresh = await Quote(hub, bypass: true);
        Assert.Equal(2, fresh.Data!.Value.GetProperty("n").GetInt32());
        Assert.Null(fresh.CachedAgeMs);
        Assert.Equal(2, (await Quote(hub)).Data!.Value.GetProperty("n").GetInt32());
        Assert.Equal(2, app.Runs);

        // 资源声明了 cache：第二次读取不再到达 App。
        var r1 = await hub.ReadResourceAsync("app-mcp://quote/quotes");
        var r2 = await hub.ReadResourceAsync("app-mcp://quote/quotes");
        Assert.Equal(r1.Text, r2.Text);
        Assert.Equal(1, app.Reads);

        var cache = hub.Status().Cache!;
        Assert.Equal(2, cache.Entries);
        Assert.Equal(3UL, cache.Hits);
        Assert.Equal(2UL, cache.Misses);
        Assert.True(cache.Bytes > 0);
        Assert.Equal(0UL, cache.Evictions);
    }

    [Fact]
    public async Task MaxEntriesZeroDisablesCache()
    {
        await using var hub = AppMcpHub.Start(new HubOptions
        {
            Listen = "127.0.0.1:0",
            DisableIpc = true,
            Dispatcher = null,
            ResultCache = new HubResultCacheLimits { MaxEntries = 0 },
        });
        await using var app = new QuoteApp(hub);
        await WaitTool(hub, "quote.get");
        for (var n = 1; n <= 2; n++)
        {
            var outcome = await Quote(hub);
            Assert.Equal(n, outcome.Data!.Value.GetProperty("n").GetInt32());
            Assert.Null(outcome.CachedAgeMs);
        }
        Assert.Equal(new CacheStatusInfo(0, 0, 0, 0, 0), hub.Status().Cache);
    }

    [Fact]
    public void NegativeLimitRejectedAndJsonWritten()
    {
        Assert.Throws<ArgumentOutOfRangeException>(() => AppMcpHub.Start(new HubOptions
        {
            DisableIpc = true,
            DisableListen = true,
            Dispatcher = null,
            ResultCache = new HubResultCacheLimits { MaxBytes = -1 },
        }));
        var json = new HubResultCacheLimits { MaxEntries = 8, MaxEntryBytes = 512 }.ToJson().ToJsonString();
        Assert.Equal("""{"maxEntries":8,"maxEntryBytes":512}""", json);
    }
}
