using System.Collections.Concurrent;
using System.Runtime.InteropServices;
using System.Text.Json.Nodes;
using AppMcp.Activation;
using AppMcp.Native;
using Xunit.Abstractions;

namespace AppMcp.Tests;

public class LifecycleUnitTests
{
    private static AppMcpClient NewClient(LifecycleOptions? lifecycle = null) => AppMcpClient.Create(new AppMcpClientOptions
    {
        AppId = "dotnet-lc",
        AppName = "LC",
        HostUrl = "ws://127.0.0.1:1", // 不可达：连接很快失败
        Dispatcher = null,
        Lifecycle = lifecycle,
        ConnectTimeout = TimeSpan.FromMilliseconds(500),
    });

    [Fact]
    public void NativeStructLayoutsMatchHeader()
    {
        // 与 app_mcp.h 一致（64 位）：AmLifecycle = int, 3×u64, int, int, ptr, bool；AmClientOptions = u32, ptr, u32, ptr。
        if (IntPtr.Size != 8) return;
        Assert.Equal(56, Marshal.SizeOf<AmLifecycle>());
        Assert.Equal(8, (int)Marshal.OffsetOf<AmLifecycle>(nameof(AmLifecycle.IdleTimeoutMs)));
        Assert.Equal(32, (int)Marshal.OffsetOf<AmLifecycle>(nameof(AmLifecycle.Residency)));
        Assert.Equal(40, (int)Marshal.OffsetOf<AmLifecycle>(nameof(AmLifecycle.WakeTarget)));
        Assert.Equal(48, (int)Marshal.OffsetOf<AmLifecycle>(nameof(AmLifecycle.WakeBackground)));
        Assert.Equal(32, Marshal.SizeOf<AmClientOptions>());
        Assert.Equal(8, (int)Marshal.OffsetOf<AmClientOptions>(nameof(AmClientOptions.Lifecycle)));
        Assert.Equal(24, (int)Marshal.OffsetOf<AmClientOptions>(nameof(AmClientOptions.OnIdleExit)));
    }

    [Fact]
    public void LifecycleConversion()
    {
        using var strings = new Utf8Strings();
        var d = AppMcpClient.ToNative(new LifecycleOptions(), strings);
        Assert.Equal(0, d.Mode);
        Assert.Equal(60_000UL, d.IdleTimeoutMs);
        Assert.Equal(15_000UL, d.HiddenIdleTimeoutMs);
        Assert.Equal(10_000UL, d.GraceMs);
        Assert.Equal(-1, d.WakeKind);
        Assert.Equal(0, d.WakeTarget);

        var n = AppMcpClient.ToNative(new LifecycleOptions
        {
            Mode = LifecycleMode.OnDemand,
            IdleTimeout = TimeSpan.FromMilliseconds(300),
            HiddenIdleTimeout = TimeSpan.Zero,
            Residency = Residency.ExitWhenIdle,
            Wake = new WakeDescriptor(WakeKind.Aumid, "Pkg!App", true),
        }, strings);
        Assert.Equal(2, n.Mode);
        Assert.Equal(300UL, n.IdleTimeoutMs);
        Assert.Equal(0UL, n.HiddenIdleTimeoutMs);
        Assert.Equal(1, n.Residency);
        Assert.Equal(2, n.WakeKind);
        Assert.Equal("Pkg!App", Marshal.PtrToStringUTF8(n.WakeTarget));
        Assert.Equal(1, n.WakeBackground);

        Assert.Throws<ArgumentOutOfRangeException>(() => AppMcpClient.ToNative(new LifecycleOptions { Grace = TimeSpan.FromSeconds(-1) }, strings));
        Assert.Equal(0u, AppMcpClient.ToMillis32(null));
        Assert.Equal(1500u, AppMcpClient.ToMillis32(TimeSpan.FromMilliseconds(1500)));
        Assert.Throws<ArgumentOutOfRangeException>(() => AppMcpClient.ToMillis32(TimeSpan.Zero));
    }

    [Theory]
    [InlineData("app-mcp-wake:abc123", "abc123")]
    [InlineData("myapp:app-mcp/wake?token=xyz", "xyz")]
    [InlineData("myapp://app-mcp/wake?token=t-1", "t-1")]
    [InlineData("--flag", null)]
    [InlineData("myapp://other?token=x", null)]
    public void ParsesWakeTokens(string args, string? expected)
    {
        Assert.Equal(expected, AppMcpClient.ParseWakeToken(args));
    }

    [Fact]
    public void IsWakeArgs()
    {
        Assert.True(AppMcpClient.IsWakeArgs(["--x", "app-mcp-wake:t"]));
        Assert.False(AppMcpClient.IsWakeArgs(["--x"]));
    }

    [Fact]
    public async Task OnDemandStartsDormantAndWakesOnHandleWake()
    {
        using var client = NewClient(new LifecycleOptions { Mode = LifecycleMode.OnDemand });
        var states = new ConcurrentQueue<ClientStatus>();
        client.StateChanged += (_, e) => states.Enqueue(e.State.Status);
        client.Start();
        await WaitUntil(() => client.State.Status == ClientStatus.Dormant);
        Assert.False(client.HandleWake("not-a-wake"));
        Assert.False(client.HandleWake(["a", "b"]));
        Assert.True(client.HandleWake(["--foo", "app-mcp-wake:tok"]));
        await WaitUntil(() => client.State.Status != ClientStatus.Dormant);
        Assert.Contains(ClientStatus.Dormant, states);
    }

    [Fact]
    public void HoldSleepWakeAndHash()
    {
        using var client = NewClient(new LifecycleOptions { Mode = LifecycleMode.Idle });
        var h0 = client.ToolsHash;
        Assert.Matches("^[0-9a-f]{16}$", h0);
        using (client.RegisterTool("t.one", "一", (_, _) => Task.FromResult<object?>(null)))
        {
            Assert.NotEqual(h0, client.ToolsHash);
        }
        var hold = client.Hold();
        hold.Dispose();
        hold.Dispose(); // 幂等
        _ = client.Sleep();
        _ = client.Wake();
        _ = client.ConnectNow();
        client.Stop();
    }

    [Fact]
    public void ToolCallExceptionCarriesDetails()
    {
        var e = new ToolCallException(ToolErrorKind.UserRejected, "no", new { a = 1 });
        Assert.NotNull(e.Details);
        Assert.Null(new ToolCallException(ToolErrorKind.UserRejected, "no").Details);
    }

    internal static async Task WaitUntil(Func<bool> cond, int timeoutMs = 5000)
    {
        var deadline = DateTime.UtcNow.AddMilliseconds(timeoutMs);
        while (!cond())
        {
            if (DateTime.UtcNow > deadline) throw new TimeoutException("条件未在期限内满足");
            await Task.Delay(20);
        }
    }
}

public class ActivationHelperTests
{
    private sealed class FakeRegistry : IRegistryWriter
    {
        public readonly Dictionary<(string, string), string> Values = new();
        public void SetValue(string subKeyPath, string? name, string value) => Values[(subKeyPath, name ?? "")] = value;
        public string? GetValue(string subKeyPath, string? name) => Values.GetValueOrDefault((subKeyPath, name ?? ""));
        public void DeleteTree(string subKeyPath)
        {
            foreach (var k in Values.Keys.Where(k => k.Item1 == subKeyPath || k.Item1.StartsWith(subKeyPath + @"\")).ToList()) Values.Remove(k);
        }
    }

    private sealed class FakeIdentity(string? aumid) : IPackageIdentity
    {
        public string? GetAppUserModelId() => aumid;
    }

    [Fact]
    public void RegistersProtocolInHkcuClasses()
    {
        var reg = new FakeRegistry();
        ProtocolRegistration.Register("myapp", @"C:\Apps\My App.exe", "My App", reg);
        Assert.Equal("URL:My App", reg.Values[(@"Software\Classes\myapp", "")]);
        Assert.Equal("", reg.Values[(@"Software\Classes\myapp", "URL Protocol")]);
        Assert.Equal("\"C:\\Apps\\My App.exe\" \"%1\"", reg.Values[(@"Software\Classes\myapp\shell\open\command", "")]);
        Assert.True(ProtocolRegistration.IsRegistered("myapp", @"C:\Apps\My App.exe", reg));
        Assert.False(ProtocolRegistration.IsRegistered("myapp", @"C:\Other.exe", reg));
        ProtocolRegistration.Unregister("myapp", reg);
        Assert.Empty(reg.Values);
        Assert.False(ProtocolRegistration.IsRegistered("myapp", @"C:\Apps\My App.exe", reg));
    }

    /// <summary>Windows：写入真实 HKCU（测试专用 scheme），读回后删除。其他平台直接返回。</summary>
    [Fact]
    public void RealHkcuRegistrationOnWindows()
    {
        if (!OperatingSystem.IsWindows()) return;
        var scheme = $"appmcp-unittest-{Environment.ProcessId}";
        var exe = @"C:\Program Files\App MCP Test\Test App.exe";
        try
        {
            ProtocolRegistration.Register(scheme, exe, "App MCP 单元测试");
            Assert.True(ProtocolRegistration.IsRegistered(scheme, exe));
            using (var key = Microsoft.Win32.Registry.CurrentUser.OpenSubKey($@"Software\Classes\{scheme}"))
            {
                Assert.NotNull(key);
                Assert.Equal("URL:App MCP 单元测试", key.GetValue(""));
                Assert.Equal("", key.GetValue("URL Protocol"));
            }
            using (var cmd = Microsoft.Win32.Registry.CurrentUser.OpenSubKey($@"Software\Classes\{scheme}\shell\open\command"))
            {
                Assert.Equal($"\"{exe}\" \"%1\"", cmd?.GetValue(""));
            }
            Assert.False(ProtocolRegistration.IsRegistered(scheme, @"C:\Other.exe"));
        }
        finally
        {
            ProtocolRegistration.Unregister(scheme);
        }
        Assert.Null(Microsoft.Win32.Registry.CurrentUser.OpenSubKey($@"Software\Classes\{scheme}"));
        Assert.False(ProtocolRegistration.IsRegistered(scheme, exe));
        // 未打包的测试进程没有包身份：走真实 kernel32 调用，应返回 null。
        Assert.Null(new WindowsPackageIdentity().GetAppUserModelId());
        Assert.Equal(new WakeDescriptor(WakeKind.Uri, scheme), WakeDescriptorFactory.ForWindows(scheme));
    }

    [Fact]
    public void RejectsBadSchemeAndNonWindowsDefault()
    {
        var reg = new FakeRegistry();
        Assert.Throws<ArgumentException>(() => ProtocolRegistration.Register("1bad", "x.exe", registry: reg));
        Assert.Throws<ArgumentException>(() => ProtocolRegistration.Register("bad scheme", "x.exe", registry: reg));
        if (!OperatingSystem.IsWindows())
        {
            Assert.Throws<PlatformNotSupportedException>(() => ProtocolRegistration.Register("myapp", "x.exe"));
        }
    }

    [Fact]
    public void WakeDescriptorPrefersAumid()
    {
        Assert.Equal(new WakeDescriptor(WakeKind.Aumid, "Co.App_abc!App", false),
            WakeDescriptorFactory.ForWindows("myapp", identity: new FakeIdentity("Co.App_abc!App")));
        Assert.Equal(new WakeDescriptor(WakeKind.Uri, "myapp", true),
            WakeDescriptorFactory.ForWindows("myapp", background: true, identity: new FakeIdentity(null)));
        Assert.Equal(WakeKind.None, WakeDescriptorFactory.ForWindows(null, identity: new FakeIdentity(null)).Kind);
        if (!OperatingSystem.IsWindows()) Assert.Null(new WindowsPackageIdentity().GetAppUserModelId());
    }

    [Fact]
    public void PipeNamesAreShortAndStable()
    {
        var (m1, p1) = SingleInstance.Names("some-very-long-application-key-" + new string('x', 200));
        var (m2, p2) = SingleInstance.Names("some-very-long-application-key-" + new string('x', 200));
        Assert.Equal(p1, p2);
        Assert.Equal(m1, m2);
        Assert.True(p1.Length < 40);
        Assert.NotEqual(p1, SingleInstance.Names("other").Pipe);
    }

    [Fact]
    public async Task SecondInstanceForwardsArgsToFirst()
    {
        var key = "it-" + Guid.NewGuid().ToString("N");
        using var first = SingleInstance.Acquire(key, []);
        Assert.NotNull(first);
        var received = new TaskCompletionSource<ActivationEventArgs>(TaskCreationOptions.RunContinuationsAsynchronously);
        first!.Activated += (_, e) => received.TrySetResult(e);

        // 第二实例在另一个线程上（模拟另一个进程；Mutex 有线程亲和性）。
        var second = await Task.Run(() => SingleInstance.Acquire(key, ["--open", "doc.txt"]));
        Assert.Null(second);
        var e = await received.Task.WaitAsync(TimeSpan.FromSeconds(10));
        Assert.Equal(["--open", "doc.txt"], e.Args);
        Assert.False(e.WakeHandled);
    }

    [Fact]
    public async Task ForwardedWakeArgsReachClient()
    {
        var key = "it-" + Guid.NewGuid().ToString("N");
        using var client = AppMcpClient.Create(new AppMcpClientOptions
        {
            AppId = "dotnet-si",
            AppName = "SI",
            HostUrl = "ws://127.0.0.1:1",
            Dispatcher = null,
            Lifecycle = new LifecycleOptions { Mode = LifecycleMode.OnDemand },
        });
        client.Start();
        await LifecycleUnitTests.WaitUntil(() => client.State.Status == ClientStatus.Dormant);

        using var first = SingleInstance.Acquire(key, []);
        Assert.NotNull(first);
        first!.AttachClient(client);
        var received = new TaskCompletionSource<ActivationEventArgs>(TaskCreationOptions.RunContinuationsAsynchronously);
        first.Activated += (_, e) => received.TrySetResult(e);

        Assert.Null(await Task.Run(() => SingleInstance.Acquire(key, ["myapp:app-mcp/wake?token=abc"])));
        var e = await received.Task.WaitAsync(TimeSpan.FromSeconds(10));
        Assert.True(e.WakeHandled);
        await LifecycleUnitTests.WaitUntil(() => client.State.Status != ClientStatus.Dormant);
    }

    [Fact]
    public async Task ForwardTimesOutWithoutListener()
    {
        await Assert.ThrowsAsync<TimeoutException>(() =>
            SingleInstance.ForwardAsync("app-mcp-nobody-" + Guid.NewGuid().ToString("N")[..8], ["x"], TimeSpan.FromMilliseconds(300)));
    }
}

/// <summary>对真实 fake_host 的生命周期往返：idle 休眠 → HandleWake 回连（跳过 sync）→ 调用 → 再休眠。</summary>
public class LifecycleIntegrationTests(ITestOutputHelper output)
{
    [Fact]
    public async Task IdleSleepWakeRoundTrip()
    {
        var fakeHost = FakeHost.Locate(output);
        if (fakeHost is null) return;

        using var host = FakeHost.Start(fakeHost,
            "--invoke", "fail.details", "--args", "{}",
            "--invoke", "fail.list", "--args", "{}",
            "--await-sleep",
            "--wake",
            "--invoke", "greet", "--args", """{"name":"Wake"}""",
            "--await-sleep",
            "--timeout-ms", "20000");
        var addr = await host.ReadListeningAsync();

        var states = new ConcurrentQueue<ClientStatus>();
        await using var client = AppMcpClient.Create(new AppMcpClientOptions
        {
            AppId = "dotnet-lc-it",
            AppName = ".NET 生命周期",
            HostUrl = $"ws://{addr}",
            Dispatcher = null,
            Lifecycle = new LifecycleOptions
            {
                Mode = LifecycleMode.Idle,
                IdleTimeout = TimeSpan.FromMilliseconds(300),
                Wake = new WakeDescriptor(WakeKind.Uri, "dotnet-lc"),
            },
        });
        client.StateChanged += (_, e) => states.Enqueue(e.State.Status);

        using var fail = client.RegisterTool("fail.details", "带详情失败", (_, _) =>
            throw new ToolCallException(ToolErrorKind.UserRejected, "额度不足", new { reason = "quota", retryAfter = 3 }));
        using var failList = client.RegisterTool("fail.list", "非对象详情", (_, _) =>
            throw new ToolCallException(ToolErrorKind.InvalidInput, "字段缺失", new[] { "a", "b" }));
        using var greet = client.RegisterTool<GreetInput, GreetOutput>("greet", "问好", (input, _) =>
            Task.FromResult(new GreetOutput($"Hello, {input.Name}!")));

        client.Start();
        var wakeLine = await host.WaitForLineAsync(l => l.Contains("\"type\":\"wake\""), TimeSpan.FromSeconds(15));
        await LifecycleUnitTests.WaitUntil(() => client.State.Status == ClientStatus.Dormant);
        var arg = (string)JsonNode.Parse(wakeLine)!["arg"]!;
        Assert.True(client.HandleWake(arg));

        var lines = await host.WaitForExitAsync(TimeSpan.FromSeconds(30));
        foreach (var l in lines) output.WriteLine(l);
        Assert.Equal(0, host.ExitCode);

        var json = lines.Where(l => l.StartsWith('{')).Select(l => JsonNode.Parse(l)!.AsObject()).ToList();
        var invokes = json.Where(j => (string?)j["type"] == "invoke").ToList();
        Assert.Equal(3, invokes.Count);
        var listData = invokes[1]["error"]!["data"]!;
        Assert.Equal("INVALID_INPUT", (string?)listData["kind"]);
        Assert.Equal(["a", "b"], listData["details"]!.AsArray().Select(x => (string?)x));
        var data = invokes[0]["error"]!["data"]!;
        Assert.Equal("USER_REJECTED", (string?)data["kind"]);
        Assert.Equal("quota", (string?)data["reason"]);
        Assert.Equal(3, (int?)data["retryAfter"]);
        Assert.Equal("Hello, Wake!", (string?)invokes[2]["result"]!["data"]!["greeting"]);

        var sleeps = json.Where(j => (string?)j["type"] == "sleep").ToList();
        Assert.Equal(2, sleeps.Count);
        Assert.All(sleeps, s => Assert.True((bool?)s["accepted"]));
        Assert.Equal("idle", (string?)sleeps[0]["reason"]);

        var hello = json.Single(j => (string?)j["type"] == "hello");
        Assert.True((bool?)hello["toolsCurrent"]);
        Assert.Equal("os-activation", (string?)hello["wakeReason"]);
        Assert.Equal(JsonNode.Parse(wakeLine)!["token"]!.ToString(), (string?)hello["launchToken"]);

        var tools = json.Where(j => (string?)j["type"] == "tools").ToList();
        Assert.Equal(2, tools.Count);
        Assert.False((bool?)tools[1]["synced"]);
        Assert.Contains("greet", tools[1]["tools"]!.AsArray().Select(t => (string?)t));

        await LifecycleUnitTests.WaitUntil(() => client.State.Status == ClientStatus.Dormant);
        Assert.Contains(ClientStatus.Waking, states);
    }
}
