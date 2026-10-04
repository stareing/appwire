using System.Collections.Concurrent;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Text.Json.Nodes;
using AppMcp.Internal;
using AppMcp.Native;
using Xunit.Abstractions;

namespace AppMcp.Tests;

/// <summary>界面级暴露与导航（spec/protocol.md 3.4，C ABI v14）。</summary>
public class NavigationTests(ITestOutputHelper output)
{
    [Fact]
    public void ToolOptionsUseV23Layout()
    {
        // @why 回归：v13 的 AmToolOptions 没有 page / surface、v14 没有 background_tool、v17 没有 concurrency / exclusive、
        //      v20 没有 implements、v21 没有 cache_ttl_ms / cache_scope、v22 没有 deprecated_*，struct_size 按旧布局传入时库不读取这些字段。
        //      32 位平台上 uint64_t 的对齐随 ABI 而异，只核对 64 位布局。
        if (IntPtr.Size == 8)
        {
            Assert.Equal(120, Unsafe.SizeOf<AmToolOptions>());
            Assert.Equal(80, (int)Marshal.OffsetOf<AmToolOptions>(nameof(AmToolOptions.CacheTtlMs)));
            Assert.Equal(88, (int)Marshal.OffsetOf<AmToolOptions>(nameof(AmToolOptions.CacheScope)));
            Assert.Equal(96, (int)Marshal.OffsetOf<AmToolOptions>(nameof(AmToolOptions.DeprecatedMessage)));
            Assert.Equal(104, (int)Marshal.OffsetOf<AmToolOptions>(nameof(AmToolOptions.DeprecatedReplacement)));
            Assert.Equal(112, (int)Marshal.OffsetOf<AmToolOptions>(nameof(AmToolOptions.DeprecatedUntil)));
        }
        using var strings = new Utf8Strings();
        var o = ToolScope.BuildOptions(strings, new ToolOptions { Surface = ToolSurface.View, Page = "cart" });
        Assert.Equal((uint)Unsafe.SizeOf<AmToolOptions>(), o.StructSize);
        Assert.Equal(1, o.Surface);
        Assert.Equal("cart", NativeMethods.PtrToString(o.Page));
        var d = ToolScope.BuildOptions(strings, new ToolOptions());
        Assert.Equal((0, (nint)0, (nint)0), (d.Surface, d.Page, d.BackgroundTool));
        var bg = ToolScope.BuildOptions(strings, new ToolOptions { Surface = ToolSurface.View, BackgroundTool = "cart.summary" });
        Assert.Equal("cart.summary", NativeMethods.PtrToString(bg.BackgroundTool));
        Assert.Equal((0u, (nint)0), (d.Concurrency, d.Exclusive));
        var sched = ToolScope.BuildOptions(strings, new ToolOptions { Concurrency = 2, Exclusive = "doc" });
        Assert.Equal(2u, sched.Concurrency);
        Assert.Equal("doc", NativeMethods.PtrToString(sched.Exclusive));
        Assert.Throws<ArgumentOutOfRangeException>(() => ToolScope.BuildOptions(strings, new ToolOptions { Concurrency = -1 }));
    }

    [Fact]
    public void InvalidExclusiveGroupIsRejectedByNativeLibrary()
    {
        // 互斥组名的规则只在原生库定义（同工具名，spec/protocol.md 5.3），封装层原样传递。
        using var client = AppMcpClient.Create(new AppMcpClientOptions
        {
            AppId = "dotnet-sched", AppName = "Sched", HostUrl = "ws://127.0.0.1:1", Dispatcher = null,
        });
        using var tool = client.RegisterTool("doc.edit", "改文档", (_, _) => Task.FromResult<object?>(null),
            new ToolOptions { Concurrency = 1, Exclusive = "doc" });
        tool.Update("改文档", new ToolOptions { Exclusive = "doc.other" });
        var ex = Assert.Throws<AppMcpException>(() => tool.Update("改文档", new ToolOptions { Exclusive = "bad group!" }));
        Assert.NotEqual(AppMcpStatus.Ok, ex.Status);
    }

    [Fact]
    public void BackgroundToolAffectsToolsHash()
    {
        using var client = AppMcpClient.Create(new AppMcpClientOptions
        {
            AppId = "dotnet-nav", AppName = "Nav", HostUrl = "ws://127.0.0.1:1", Dispatcher = null, NavigateInBackground = false,
        });
        using var tool = client.RegisterTool("cart.checkout", "结算", (_, _) => Task.FromResult<object?>(null),
            new ToolOptions { Surface = ToolSurface.View, Page = "cart" });
        var plain = client.ToolsHash;
        tool.Update("结算", new ToolOptions { Surface = ToolSurface.View, Page = "cart", BackgroundTool = "cart.summary" });
        Assert.NotEqual(plain, client.ToolsHash);
        tool.Update("结算", new ToolOptions { Surface = ToolSurface.View, Page = "cart" }); // null 清除
        Assert.Equal(plain, client.ToolsHash);
        var ex = Assert.Throws<AppMcpException>(() =>
            tool.Update("结算", new ToolOptions { Surface = ToolSurface.View, BackgroundTool = "bad tool!" }));
        Assert.NotEqual(AppMcpStatus.Ok, ex.Status);
        client.SetNavigateInBackground(true);
    }

    /// <summary>后台 + NavigateInBackground：handler 抛 <see cref="UserActionRequiredException"/> → USER_ACTION_REQUIRED（reason / uri）；
    /// 关闭后核心直接以 foreground 回复、不调用 handler。</summary>
    [Fact]
    public async Task BackgroundNavigationAnswersUserAction()
    {
        var fakeHost = FakeHost.Locate(output);
        if (fakeHost is null) return;
        using var host = FakeHost.Start(fakeHost, "--navigate", "cart", "--navigate", "home", "--timeout-ms", "20000");
        var addr = await host.ReadListeningAsync();
        var calls = 0;
        await using var client = AppMcpClient.Create(new AppMcpClientOptions
        {
            AppId = "dotnet-bg-nav", AppName = "Nav", HostUrl = $"ws://{addr}", Dispatcher = null, NavigateInBackground = true,
        });
        client.SetNavigationHandler((Action<NavigationRequest>)(request =>
        {
            Interlocked.Increment(ref calls);
            client.SetNavigateInBackground(false);
            throw new UserActionRequiredException("已发通知，请点开后继续", UserActionReason.Foreground, "conf://" + request.Page);
        }));
        client.SetVisibility(AppVisibility.Hidden, false);
        client.Start();
        var lines = await host.WaitForExitAsync(TimeSpan.FromSeconds(30));
        foreach (var l in lines) output.WriteLine(l);
        Assert.Equal(0, host.ExitCode);

        var navs = lines.Where(l => l.StartsWith('{')).Select(l => JsonNode.Parse(l)!.AsObject())
            .Where(j => (string?)j["type"] == "navigate").ToList();
        Assert.Equal(2, navs.Count);
        Assert.Equal(-32019, (int)navs[0]["error"]!["code"]!);
        Assert.Equal("已发通知，请点开后继续", (string?)navs[0]["error"]!["message"]);
        Assert.Equal("foreground", (string?)navs[0]["error"]!["data"]!["reason"]);
        Assert.Equal("conf://cart", (string?)navs[0]["error"]!["data"]!["uri"]);
        Assert.Equal(-32019, (int)navs[1]["error"]!["code"]!);
        Assert.Equal("foreground", (string?)navs[1]["error"]!["data"]!["reason"]);
        Assert.Null(navs[1]["error"]!["data"]!["uri"]);
        Assert.Equal(1, calls);
    }

    [Fact]
    public void SurfaceAndPageAffectToolsHash()
    {
        using var client = AppMcpClient.Create(new AppMcpClientOptions { AppId = "dotnet-nav", AppName = "Nav", HostUrl = "ws://127.0.0.1:1", Dispatcher = null });
        using var tool = client.RegisterTool("cart.checkout", "结算", (_, _) => Task.FromResult<object?>(null));
        var plain = client.ToolsHash;
        tool.Update("结算", new ToolOptions { Surface = ToolSurface.View, Page = "cart" });
        Assert.NotEqual(plain, client.ToolsHash);
        var ex = Assert.Throws<AppMcpException>(() =>
            client.RegisterTool("bad", "x", (_, _) => Task.FromResult<object?>(null), new ToolOptions { Page = "bad page!" }));
        Assert.NotEqual(AppMcpStatus.Ok, ex.Status);
        client.SetNavigationHandler(_ => { });
        client.SetNavigationHandler((Action<NavigationRequest>?)null);
    }

    [Fact]
    public void NavigationRequestParsesParams()
    {
        Assert.Null(new NavigationRequest("cart", null).Params);
        Assert.Equal("A-42", new NavigationRequest("detail", """{"sku":"A-42"}""").Params!.Value.GetProperty("sku").GetString());
    }

    /// <summary>handler 在调度器（UI 线程）上执行；完成 / 拒绝 / 失败 / 抛异常 → Host 看到对应结果。</summary>
    [Fact]
    public async Task NavigatesOnDispatcher()
    {
        var fakeHost = FakeHost.Locate(output);
        if (fakeHost is null) return;
        using var host = FakeHost.Start(fakeHost,
            "--navigate", "cart", "--nav-params", """{"id":7}""",
            "--navigate", "login",
            "--navigate", "crash",
            "--timeout-ms", "20000");
        var addr = await host.ReadListeningAsync();
        using var ui = new SingleThreadSynchronizationContext();
        var threads = new ConcurrentBag<int>();
        var requests = new ConcurrentQueue<NavigationRequest>();
        await using var client = AppMcpClient.Create(new AppMcpClientOptions
        {
            AppId = "dotnet-nav", AppName = "Nav", HostUrl = $"ws://{addr}", Dispatcher = ui,
        });
        client.SetNavigationHandler(async request =>
        {
            threads.Add(Environment.CurrentManagedThreadId);
            requests.Enqueue(request);
            await Task.Yield();
            if (request.Page == "login") throw new NavigationDeniedException("需要先登录");
            if (request.Page == "crash") throw new InvalidOperationException("页面崩溃");
        });
        client.Start();
        var lines = await host.WaitForExitAsync(TimeSpan.FromSeconds(30));
        foreach (var l in lines) output.WriteLine(l);
        Assert.Equal(0, host.ExitCode);

        var navs = lines.Where(l => l.StartsWith('{')).Select(l => JsonNode.Parse(l)!.AsObject())
            .Where(j => (string?)j["type"] == "navigate").ToList();
        Assert.Equal(3, navs.Count);
        Assert.True((bool)navs[0]["result"]!["ok"]!);
        Assert.Equal(-31002, (int)navs[1]["error"]!["code"]!);
        Assert.Contains("需要先登录", (string?)navs[1]["error"]!["message"]);
        Assert.Equal(-31001, (int)navs[2]["error"]!["code"]!);
        Assert.Contains("页面崩溃", (string?)navs[2]["error"]!["message"]);
        Assert.NotEmpty(threads);
        Assert.All(threads, t => Assert.Equal(ui.ThreadId, t));
        Assert.Equal("""{"id":7}""", requests.First().ParamsJson);
    }
}

public class ViewToolGateTests
{
    [Fact]
    public void EnablesOnlyWhenVisibleAndActive()
    {
        var log = new List<bool>();
        using var gate = new ViewToolGate().Add(log.Add);
        Assert.Equal([false], log);
        gate.SetVisible(true);
        Assert.Equal([false, true], log);
        gate.SetVisible(true); // 状态没变：不重复通知
        gate.SetActive(false);
        gate.SetVisible(false);
        gate.SetActive(true);
        Assert.Equal([false, true, false], log);
        gate.SetVisible(true);
        gate.Dispose();
        Assert.Equal([false, true, false, true, false], log);
        gate.SetVisible(true); // 释放后忽略
        Assert.Equal(5, log.Count);
        Assert.Throws<ObjectDisposedException>(() => gate.Add(_ => { }));
    }

    [Fact]
    public void TogglesRegisteredTools()
    {
        using var client = AppMcpClient.Create(new AppMcpClientOptions { AppId = "dotnet-gate", AppName = "Gate", HostUrl = "ws://127.0.0.1:1", Dispatcher = null });
        using var app = client.RegisterTool("app.tool", "后台工具", (_, _) => Task.FromResult<object?>(null));
        var appOnly = client.ToolsHash;
        using var view = client.RegisterTool("cart.checkout", "结算", (_, _) => Task.FromResult<object?>(null),
            new ToolOptions { Surface = ToolSurface.View, Page = "cart" });
        var withView = client.ToolsHash;
        Assert.NotEqual(appOnly, withView);
        using var gate = new ViewToolGate(visible: false).Add(view);
        Assert.Equal(appOnly, client.ToolsHash); // 禁用的工具不计入摘要
        gate.SetVisible(true);
        Assert.Equal(withView, client.ToolsHash);
        view.Dispose();
        gate.SetVisible(false); // 已注销：忽略错误
    }
}
