using System.Text.Json;
using System.Text.Json.Nodes;

namespace AppMcp.Hub.Tests;

/// <summary>对象锁（spec/hub-api.md 3.6「对象锁」）：HubOptions.MaxLocks、apps.lock / apps.unlock、HubStatusInfo.Locks、HubError.Locked。</summary>
public class LocksTests
{
    private static AppMcpHub Start(int? maxLocks)
    {
        var options = new HubOptions { DisableIpc = true, DisableListen = true, Dispatcher = null, MaxLocks = maxLocks };
        options.Manifests.Add(JsonNode.Parse("""
            {"manifestVersion":1,"appId":"shop","name":"商城",
             "tools":[{"name":"cart.add","description":"加购","inputSchema":{"type":"object"}}]}
            """)!);
        return AppMcpHub.Start(options);
    }

    private static string[] BuiltinNames(AppMcpHub hub) =>
        hub.ListTools().Select(t => t.Name).Where(n => n.StartsWith("apps.", StringComparison.Ordinal)).ToArray();

    [Fact]
    public void MaxLocksSerializes()
    {
        Assert.Equal(0, (int?)JsonNode.Parse(new HubOptions { MaxLocks = 0 }.ToConfigJson())!["maxLocks"]);
        Assert.Equal(3, (int?)JsonNode.Parse(new HubOptions { MaxLocks = 3 }.ToConfigJson())!["maxLocks"]);
        Assert.False(JsonNode.Parse(new HubOptions().ToConfigJson())!.AsObject().ContainsKey("maxLocks"));
        Assert.Throws<ArgumentOutOfRangeException>(() => new HubOptions { MaxLocks = -1 }.ToConfigJson());
    }

    /// <summary>缺省列出 apps.lock / apps.unlock；会话 s1 加锁后 s2 加同一把锁 → LOCKED；Status().Locks 列出未到期的锁。</summary>
    [Fact]
    public async Task LockConflictAndStatus()
    {
        using var hub = Start(null);
        Assert.Contains("apps.lock", BuiltinNames(hub));
        Assert.Contains("apps.unlock", BuiltinNames(hub));

        var ok = await hub.CallAsync(new CallRequest("apps.lock", new { appId = "shop", ttlMs = 30000 }) { Session = "s1" });
        Assert.True(ok.IsSuccess, ok.Json.GetRawText());
        var denied = await hub.CallAsync(new CallRequest("apps.lock", new { appId = "shop" }) { Session = "s2" });
        Assert.Equal(HubError.Locked, denied.Error?.Kind);
        Assert.Equal("api", denied.Error!.Details!.Value.GetProperty("holder").GetString());

        var l = Assert.Single(hub.Status().Locks!);
        Assert.Equal(("shop", (string?)null, "api:s1", "api"), (l.AppId, l.Key, l.Caller, l.Holder));
        Assert.InRange(l.ExpiresInMs, 1UL, 30000UL);

        var released = await hub.CallAsync(new CallRequest("apps.unlock", new { appId = "shop" }) { Session = "s1" });
        Assert.True(released.Data!.Value.GetProperty("released").GetBoolean(), released.Json.GetRawText());
        Assert.Empty(hub.Status().Locks!);
    }

    [Fact]
    public async Task MaxLocksZeroDisables()
    {
        using var hub = Start(0);
        Assert.DoesNotContain("apps.lock", BuiltinNames(hub));
        Assert.DoesNotContain("apps.unlock", BuiltinNames(hub));
        var r = await hub.CallAsync(new CallRequest("apps.lock", new { appId = "shop" }) { Session = "s1" });
        Assert.Equal("TOOL_NOT_FOUND", r.Error?.Kind);
    }

    [Fact]
    public void StatusParsesLocks()
    {
        var status = JsonSerializer.Deserialize<HubStatusInfo>("""
            {"service":"app-mcp","version":"0","pid":1,"startedAtMs":0,"mcpHttp":true,
             "auth":{"tokenConfigured":false,"tokenRequiredWithoutOrigin":false},"mcpSessions":0,"apps":[],"reports":[],
             "locks":[{"appId":"shop","caller":"api:s1","holder":"api","expiresInMs":1500},
                      {"appId":"shop","key":"doc-1","caller":"principal:agent:claude","holder":"agent:claude","expiresInMs":9}]}
            """, AppMcpHub.WireOptions)!;
        Assert.Equal(
            [new LockStatusInfo("shop", "api:s1", "api", 1500), new LockStatusInfo("shop", "principal:agent:claude", "agent:claude", 9) { Key = "doc-1" }],
            status.Locks!);
        var old = JsonSerializer.Deserialize<HubStatusInfo>("""
            {"service":"app-mcp","version":"0","pid":1,"startedAtMs":0,"mcpHttp":true,
             "auth":{"tokenConfigured":false,"tokenRequiredWithoutOrigin":false},"mcpSessions":0,"apps":[],"reports":[]}
            """, AppMcpHub.WireOptions)!;
        Assert.Null(old.Locks);
    }
}
