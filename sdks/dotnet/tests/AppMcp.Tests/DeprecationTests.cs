using AppMcp.Native;

namespace AppMcp.Tests;

/// <summary>工具弃用声明（spec/protocol.md 3.7，C ABI v23 <c>deprecated_message</c> / <c>deprecated_replacement</c> / <c>deprecated_until</c>）。</summary>
public class DeprecationTests
{
    [Fact]
    public void DeprecatedIsPassedAsThreeStrings()
    {
        using var strings = new Utf8Strings();
        var full = ToolScope.BuildOptions(strings, new ToolOptions { Deprecated = new ToolDeprecation("改用 v2", "orders.list2", "2027-06-30") });
        Assert.Equal(("改用 v2", "orders.list2", "2027-06-30"),
            (NativeMethods.PtrToString(full.DeprecatedMessage), NativeMethods.PtrToString(full.DeprecatedReplacement),
             NativeMethods.PtrToString(full.DeprecatedUntil)));
        var minimal = ToolScope.BuildOptions(strings, new ToolOptions { Deprecated = new ToolDeprecation("即将移除") });
        Assert.Equal("即将移除", NativeMethods.PtrToString(minimal.DeprecatedMessage));
        Assert.Equal(((nint)0, (nint)0), (minimal.DeprecatedReplacement, minimal.DeprecatedUntil));
        var none = ToolScope.BuildOptions(strings, new ToolOptions());
        Assert.Equal(((nint)0, (nint)0, (nint)0), (none.DeprecatedMessage, none.DeprecatedReplacement, none.DeprecatedUntil));
    }

    [Fact]
    public void DeprecatedAffectsToolsHashAndInvalidIsRejected()
    {
        using var client = AppMcpClient.Create(new AppMcpClientOptions
        {
            AppId = "dotnet-deprecated", AppName = "Deprecated", HostUrl = "ws://127.0.0.1:1", Dispatcher = null,
        });
        using var tool = client.RegisterTool("orders.list", "列出订单", (_, _) => Task.FromResult<object?>(null));
        var plain = client.ToolsHash;
        tool.Update("列出订单", new ToolOptions { Deprecated = new ToolDeprecation("改用 orders.list2", "orders.list2", "2027-06-30") });
        var withFull = client.ToolsHash;
        Assert.NotEqual(plain, withFull);
        // 格式只在原生库校验（spec/protocol.md 3.7），封装层原样传递。
        foreach (var bad in new[]
        {
            new ToolDeprecation(""), new ToolDeprecation("m", "orders.list"), new ToolDeprecation("m", Until: "2027-02-29"),
            new ToolDeprecation(new string('字', 501)),
        })
        {
            var ex = Assert.Throws<AppMcpException>(() => tool.Update("列出订单", new ToolOptions { Deprecated = bad }));
            Assert.Equal(AppMcpStatus.InvalidConfig, ex.Status);
        }
        Assert.Equal(withFull, client.ToolsHash);
        tool.Update("列出订单", new ToolOptions { Deprecated = new ToolDeprecation("改用 orders.list2") });
        Assert.NotEqual(withFull, client.ToolsHash);
        tool.Update("列出订单", new ToolOptions()); // null 清除
        Assert.Equal(plain, client.ToolsHash);
    }
}
