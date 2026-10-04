using AppMcp.Internal;
using AppMcp.Native;

namespace AppMcp.Tests;

/// <summary>工具声明的标准意图（spec/intents.md，C ABI v21 <c>AmToolOptions.implements</c>）。</summary>
public class IntentsTests
{
    [Fact]
    public void ImplementsArePassedAsNativeStringArray()
    {
        using var strings = new Utf8Strings();
        var o = ToolScope.BuildOptions(strings, new ToolOptions { Implements = ["link.open@1", "file.share@1"] });
        Assert.Equal((nuint)2, o.ImplementsLen);
        Assert.Equal("link.open@1", NativeMethods.PtrToString(System.Runtime.InteropServices.Marshal.ReadIntPtr(o.Implements, 0)));
        Assert.Equal("file.share@1", NativeMethods.PtrToString(System.Runtime.InteropServices.Marshal.ReadIntPtr(o.Implements, IntPtr.Size)));
        foreach (var empty in new[] { new ToolOptions(), new ToolOptions { Implements = [] } })
        {
            var d = ToolScope.BuildOptions(strings, empty);
            Assert.Equal(((nint)0, (nuint)0), (d.Implements, d.ImplementsLen));
        }
    }

    [Fact]
    public void ImplementsAffectToolsHashAndInvalidVerbIsRejected()
    {
        using var client = AppMcpClient.Create(new AppMcpClientOptions
        {
            AppId = "dotnet-intents", AppName = "Intents", HostUrl = "ws://127.0.0.1:1", Dispatcher = null,
        });
        using var tool = client.RegisterTool("open", "打开链接", (_, _) => Task.FromResult<object?>(null));
        var plain = client.ToolsHash;
        tool.Update("打开链接", new ToolOptions { Implements = ["link.open@1"] });
        Assert.NotEqual(plain, client.ToolsHash);
        // 动词格式的规则只在原生库定义（spec/intents.md 第 1 节），封装层原样传递。
        var ex = Assert.Throws<AppMcpException>(() => tool.Update("打开链接", new ToolOptions { Implements = ["link.open"] }));
        Assert.Equal(AppMcpStatus.InvalidName, ex.Status);
        tool.Update("打开链接", new ToolOptions()); // null 清除
        Assert.Equal(plain, client.ToolsHash);
    }
}
