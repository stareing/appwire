using AppMcp.Native;

namespace AppMcp.Tests;

/// <summary>工具 / 资源的结果缓存声明（spec/protocol.md 3.6，C ABI v22 <c>cache_ttl_ms</c> / <c>cache_scope</c>）。</summary>
public class CacheTests
{
    [Fact]
    public void CacheIsPassedAsTtlAndScope()
    {
        using var strings = new Utf8Strings();
        var shared = ToolScope.BuildOptions(strings, new ToolOptions { Cache = new CachePolicy(60_000, CacheScope.Shared) });
        Assert.Equal((60_000UL, 1), (shared.CacheTtlMs, shared.CacheScope));
        var priv = ToolScope.BuildOptions(strings, new ToolOptions { Cache = new CachePolicy(5) });
        Assert.Equal((5UL, 0), (priv.CacheTtlMs, priv.CacheScope));
        var none = ToolScope.BuildOptions(strings, new ToolOptions());
        Assert.Equal((0UL, 0), (none.CacheTtlMs, none.CacheScope));
    }

    [Fact]
    public void CacheAffectsToolsHashAndOutOfRangeIsRejected()
    {
        using var client = AppMcpClient.Create(new AppMcpClientOptions
        {
            AppId = "dotnet-cache", AppName = "Cache", HostUrl = "ws://127.0.0.1:1", Dispatcher = null,
        });
        var readOnly = new ToolOptions { Risk = ToolRisk.Read };
        using var tool = client.RegisterTool("quote", "查询报价", (_, _) => Task.FromResult<object?>(null), readOnly);
        var plain = client.ToolsHash;
        var cached = new ToolOptions { Risk = ToolRisk.Read, Cache = new CachePolicy(60_000, CacheScope.Shared) };
        tool.Update("查询报价", cached);
        var withCache = client.ToolsHash;
        Assert.NotEqual(plain, withCache);
        // ttl 范围只在原生库校验（spec/protocol.md 3.6），封装层原样传递。
        var tooLong = new ToolOptions { Risk = ToolRisk.Read, Cache = new CachePolicy(86_400_001) };
        var ex = Assert.Throws<AppMcpException>(() => tool.Update("查询报价", tooLong));
        Assert.Equal(AppMcpStatus.InvalidConfig, ex.Status);
        Assert.Equal(withCache, client.ToolsHash);
        tool.Update("查询报价", readOnly); // null 清除
        Assert.Equal(plain, client.ToolsHash);

        // 资源：越界 InvalidConfig；声明进 toolsHash（与同名未声明的注册比较）。
        var rex = Assert.Throws<AppMcpException>(() => client.RegisterResource(
            "quotes", "报价", _ => Task.FromResult<object?>(null), cache: new CachePolicy(0)));
        Assert.Equal(AppMcpStatus.InvalidConfig, rex.Status);
        string without;
        using (client.RegisterResource("quotes", "报价", _ => Task.FromResult<object?>(null))) without = client.ToolsHash;
        using var res = client.RegisterResource("quotes", "报价", _ => Task.FromResult<object?>(null),
            cache: new CachePolicy(30_000, CacheScope.Shared));
        Assert.NotEqual(without, client.ToolsHash);
    }
}
