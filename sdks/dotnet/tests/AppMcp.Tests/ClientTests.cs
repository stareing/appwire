using System.Text.Json;
using System.Text.Json.Nodes;

namespace AppMcp.Tests;

public sealed record GreetInput(string Name);

public sealed record GreetOutput(string Greeting);

public class BasicTests
{
    [Fact]
    public void NativeLibraryLoads()
    {
        Assert.False(string.IsNullOrEmpty(AppMcpClient.Version));
    }

    [Theory]
    [InlineData(ToolErrorKind.UserRejected, "USER_REJECTED")]
    [InlineData(ToolErrorKind.HandlerError, "HANDLER_ERROR")]
    [InlineData(ToolErrorKind.InvalidInput, "INVALID_INPUT")]
    [InlineData(ToolErrorKind.UnsupportedProtocol, "UNSUPPORTED_PROTOCOL")]
    [InlineData(ToolErrorKind.RateLimited, "RATE_LIMITED")]
    [InlineData(ToolErrorKind.PayloadTooLarge, "PAYLOAD_TOO_LARGE")]
    [InlineData(ToolErrorKind.PolicyDenied, "POLICY_DENIED")]
    [InlineData(ToolErrorKind.UserActionRequired, "USER_ACTION_REQUIRED")]
    public void ErrorKindStrings(ToolErrorKind kind, string expected)
    {
        Assert.Equal(expected, kind.ToProtocolString());
    }

    [Fact]
    public void UserActionRequiredExceptionCarriesReasonAndUri()
    {
        ToolCallException e = new UserActionRequiredException("请先登录", UserActionReason.Login);
        Assert.Equal(ToolErrorKind.UserActionRequired, e.Kind);
        Assert.Equal("请先登录", e.Message);
        var ua = Assert.IsType<UserActionRequiredException>(e);
        Assert.Equal("login", ua.Reason);
        Assert.Null(ua.Uri);
        Assert.Null(ua.Details);
    }

    [Fact]
    public void AnnotationsSerializeToProtocolJson()
    {
        Assert.Null(AppMcp.Internal.AnnotationsJson.Serialize((ToolAnnotations?)null));
        Assert.Equal("{}", AppMcp.Internal.AnnotationsJson.Serialize(new ToolAnnotations()));
        Assert.Equal(
            """{"title":"查询","readOnlyHint":true,"openWorldHint":false}""",
            AppMcp.Internal.AnnotationsJson.Serialize(new ToolAnnotations { Title = "查询", ReadOnlyHint = true, OpenWorldHint = false }));
        var content = new ContentAnnotations
        {
            Audience = [ContentAudience.User, ContentAudience.Assistant],
            Priority = 0.5,
            LastModified = "2026-10-02T00:00:00Z",
        };
        Assert.Equal(
            """{"audience":["user","assistant"],"priority":0.5,"lastModified":"2026-10-02T00:00:00Z"}""",
            AppMcp.Internal.AnnotationsJson.Serialize(content));
    }

    [Fact]
    public void ToolOptionsAreValidatedOnRegister()
    {
        using var client = AppMcpClient.Create(new AppMcpClientOptions
        {
            AppId = "dotnet-options",
            AppName = "Options",
            HostUrl = "ws://127.0.0.1:1",
            Dispatcher = null,
        });
        var options = new ToolOptions
        {
            Annotations = new ToolAnnotations { ReadOnlyHint = true },
            OutputSchemaJson = """{"type":"object"}""",
        };
        using var tool = client.RegisterTool("with.options", "带选项", (_, _) => Task.FromResult<object?>(new ToolResult()), options);
        var bad = new ToolOptions { OutputSchemaJson = "{" };
        var e = Assert.Throws<AppMcpException>(() => client.RegisterTool("bad.output", "x", (_, _) => Task.FromResult<object?>(null), bad));
        Assert.Equal(AppMcpStatus.InvalidSchema, e.Status);
        Assert.Equal(AppMcpStatus.InvalidSchema, Assert.Throws<AppMcpException>(() => tool.Update("改", bad)).Status);
        tool.Update("改"); // 清除注解与 outputSchema
    }

    [Fact]
    public void ApiVersionMatchesHeader()
    {
        var dir = new DirectoryInfo(AppContext.BaseDirectory);
        while (dir is not null && !File.Exists(Path.Combine(dir.FullName, "bindings", "c", "include", "app_mcp.h"))) dir = dir.Parent;
        Assert.NotNull(dir);
        var header = File.ReadAllText(Path.Combine(dir!.FullName, "bindings", "c", "include", "app_mcp.h"));
        Assert.Contains($"#define AM_API_VERSION {AppMcp.Native.NativeMethods.ApiVersion}", header);
    }

    [Fact]
    public void StateStatusMatchesHeader()
    {
        var dir = new DirectoryInfo(AppContext.BaseDirectory);
        while (dir is not null && !File.Exists(Path.Combine(dir.FullName, "bindings", "c", "include", "app_mcp.h"))) dir = dir.Parent;
        var header = File.ReadAllText(Path.Combine(dir!.FullName, "bindings", "c", "include", "app_mcp.h"));
        Assert.Contains($"AM_STATE_HOST_MISMATCH = {(int)ClientStatus.HostMismatch}", header);
        Assert.Contains($"AM_STATE_WAKING = {(int)ClientStatus.Waking}", header);
    }

    [Fact]
    public void SchemaFromType()
    {
        var schema = JsonNode.Parse(ToolSchema.For<GreetInput>())!.AsObject();
        Assert.Equal("object", schema["type"]!.GetValue<string>());
        Assert.NotNull(schema["properties"]!["name"]);
        Assert.Equal("string", schema["properties"]!["name"]!["type"]!.GetValue<string>());
    }

    [Fact]
    public void SchemaRejectsNonObject()
    {
        Assert.Throws<ArgumentException>(() => ToolSchema.For<int>());
    }

    [Fact]
    public void SchemaUsesCustomOptions()
    {
        var options = new JsonSerializerOptions { PropertyNamingPolicy = JsonNamingPolicy.SnakeCaseLower };
        var schema = JsonNode.Parse(ToolSchema.For<GreetOutputWithLongName>(options))!.AsObject();
        Assert.NotNull(schema["properties"]!["long_name"]);
    }

    public sealed record GreetOutputWithLongName(string LongName);
}

public class ClientTests
{
    internal static AppMcpClientOptions Options(string? hostUrl = "ws://127.0.0.1:1") => new()
    {
        AppId = "dotnet-test",
        AppName = ".NET Test",
        HostUrl = hostUrl,
        Dispatcher = null,
    };

    [Fact]
    public void InvalidAppIdThrows()
    {
        var e = Assert.Throws<AppMcpException>(() => AppMcpClient.Create(new AppMcpClientOptions
        {
            AppId = "Bad App Id",
            AppName = "x",
        }));
        Assert.Equal(AppMcpStatus.InvalidConfig, e.Status);
        Assert.Contains("app_id", e.Message);
    }

    [Fact]
    public void CreateWithOverview()
    {
        using var client = AppMcpClient.Create(new AppMcpClientOptions
        {
            AppId = "dotnet-test",
            AppName = ".NET Test",
            HostUrl = "ws://127.0.0.1:1",
            Overview = new AppOverview("示例 App", "## 能力\n- 问好", "zh-CN"),
        });
        Assert.Equal(ClientStatus.Idle, client.State.Status);
    }

    /// <summary>不存在的本地 IPC 端点（连接被拒绝 / 不存在 → HOST_NOT_RUNNING）。
    /// @why 不用 ws://127.0.0.1:1：WSL 等环境下回环连接未监听端口可能超时（CONNECT_TIMEOUT）。</summary>
    private static string MissingEndpoint() => OperatingSystem.IsWindows()
        ? @"pipe:\\.\pipe\app-mcp-dotnet-test-missing"
        : "unix:/nonexistent-app-mcp-dotnet-test/hub.sock";

    [Fact]
    public async Task BackoffCarriesCode()
    {
        using var client = AppMcpClient.Create(Options(MissingEndpoint()));
        Assert.Null(client.State.Code); // Idle 不带码
        Assert.Null(client.ConnectionId);

        var backoff = new TaskCompletionSource<ClientState>(TaskCreationOptions.RunContinuationsAsynchronously);
        var codeOutsideErrorStates = false;
        client.StateChanged += (_, e) =>
        {
            if (e.State.Status == ClientStatus.Backoff) backoff.TrySetResult(e.State);
            else if (!ClientState.StatusHasCode(e.State.Status) && e.State.Code is not null) codeOutsideErrorStates = true;
        };
        client.Start();

        var state = await backoff.Task.WaitAsync(TimeSpan.FromSeconds(10));
        Assert.Equal("HOST_NOT_RUNNING", state.Code);
        Assert.False(string.IsNullOrEmpty(state.Reason));
        Assert.NotNull(state.RetryIn);
        Assert.False(codeOutsideErrorStates);

        var now = client.State;
        if (now.Status == ClientStatus.Backoff) Assert.Equal("HOST_NOT_RUNNING", now.Code);
        Assert.Null(client.ConnectionId); // 从未连上

        client.Stop();
        Assert.Equal(ClientStatus.Stopped, client.State.Status);
        Assert.Null(client.State.Code);
    }

    [Fact]
    public void StateCodeIsPartOfEquality()
    {
        var a = new ClientState(ClientStatus.Backoff, TimeSpan.FromSeconds(1), "r") { Code = "HOST_NOT_RUNNING" };
        Assert.Equal(a, a with { });
        Assert.NotEqual(a, a with { Code = null });
        var (status, retryIn, reason) = a; // @compat 三元解构保持可用
        Assert.Equal((ClientStatus.Backoff, TimeSpan.FromSeconds(1), "r"), (status, retryIn, reason));
    }

    /// <summary>用户正在操作（spec/protocol.md 5.3，app_mcp.h v19）：状态读写、作用域与策略（非法值由原生库拒绝）。</summary>
    [Fact]
    public void BusyStateScopeAndPolicy()
    {
        Assert.Equal(BusyPolicy.Reject, Options().BusyPolicy);
        var bad = Assert.Throws<AppMcpException>(() => AppMcpClient.Create(new AppMcpClientOptions
        {
            AppId = "dotnet-test",
            AppName = ".NET Test",
            HostUrl = "ws://127.0.0.1:1",
            BusyPolicy = (BusyPolicy)9,
        }));
        Assert.Equal(AppMcpStatus.InvalidArgument, bad.Status);

        using var client = AppMcpClient.Create(new AppMcpClientOptions
        {
            AppId = "dotnet-test",
            AppName = ".NET Test",
            HostUrl = "ws://127.0.0.1:1",
            BusyPolicy = BusyPolicy.Queue,
        });
        Assert.False(client.IsBusy);
        client.SetBusy(true);
        Assert.True(client.IsBusy);
        client.SetBusy(false);
        Assert.False(client.IsBusy);
        client.SetBusyPolicy(BusyPolicy.Reject);
        Assert.Equal(AppMcpStatus.InvalidArgument, Assert.Throws<AppMcpException>(() => client.SetBusyPolicy((BusyPolicy)9)).Status);

        // 作用域：引用计数；有效 busy = 显式开关 ∨ 作用域数 > 0。
        var scope = client.Busy();
        Assert.True(client.IsBusy);
        scope.Dispose();
        Assert.False(client.IsBusy);

        // 嵌套：先结束一个仍为 busy；重复 Dispose 不多减。
        var outer = client.Busy();
        var inner = client.Busy();
        inner.Dispose();
        inner.Dispose();
        Assert.True(client.IsBusy);
        outer.Dispose();
        Assert.False(client.IsBusy);

        // 开关与作用域互不清除。
        var held = client.Busy();
        client.SetBusy(false);
        Assert.True(client.IsBusy); // SetBusy(false) 不结束作用域
        client.SetBusy(true);
        held.Dispose();
        Assert.True(client.IsBusy); // 作用域结束不清开关
        client.SetBusy(false);
        Assert.False(client.IsBusy);

        // 跨线程结束作用域。
        var threaded = client.Busy();
        var thread = new Thread(threaded.Dispose);
        thread.Start();
        thread.Join();
        Assert.False(client.IsBusy);

        var late = client.Busy();
        client.Dispose();
        late.Dispose(); // 客户端已释放：不抛出
    }

    [Fact]
    public void QueriesThrowAfterDispose()
    {
        var client = AppMcpClient.Create(Options());
        client.Dispose();
        Assert.Throws<ObjectDisposedException>(() => client.ConnectionId);
    }

    [Fact]
    public void BasicLifecycle()
    {
        using var client = AppMcpClient.Create(Options());
        Assert.Equal(ClientStatus.Idle, client.State.Status);
        Assert.False(string.IsNullOrEmpty(client.InstanceId));
        Assert.Null(client.Token);

        Task<GreetOutput> Handler(GreetInput i, ToolContext c) => Task.FromResult(new GreetOutput("hi " + i.Name));

        using var greet = client.RegisterTool<GreetInput, GreetOutput>("greet", "问好", Handler);
        Assert.Equal("greet", greet.Name);
        var dup = Assert.Throws<AppMcpException>(() => client.RegisterTool<GreetInput, GreetOutput>("greet", "重名", Handler));
        Assert.Equal(AppMcpStatus.DuplicateName, dup.Status);
        var bad = Assert.Throws<AppMcpException>(() => client.RegisterTool<GreetInput, GreetOutput>("bad name!", "非法", Handler));
        Assert.Equal(AppMcpStatus.InvalidName, bad.Status);
        var schema = Assert.Throws<AppMcpException>(() => client.RegisterTool(
            "s1", "schema", (_, _) => Task.FromResult<object?>(null), new ToolOptions { InputSchemaJson = """{"type":"string"}""" }));
        Assert.Equal(AppMcpStatus.InvalidSchema, schema.Status);

        greet.SetEnabled(false);
        greet.Update("新描述", new ToolOptions { Risk = ToolRisk.Read, Activation = ToolActivation.Background });

        // 作用域：Dispose 后同名工具可重新注册。
        var scope = client.CreateScope("page");
        scope.RegisterTool("page.action", "页面动作", (_, _) => Task.FromResult<object?>(null));
        scope.Dispose();
        using var again = client.RegisterTool("page.action", "页面动作", (_, _) => Task.FromResult<object?>(null));

        // 工具 Dispose 后同名可重新注册。
        greet.Dispose();
        using var greet2 = client.RegisterTool<GreetInput, GreetOutput>("greet", "问好", Handler);

        using var res = client.RegisterResource("app.state", "状态", _ => Task.FromResult<object?>(new { ok = true }));
        res.NotifyChanged();
        var dupRes = Assert.Throws<AppMcpException>(() => client.RegisterResource("app.state", "重名", _ => Task.FromResult<object?>(null)));
        Assert.Equal(AppMcpStatus.DuplicateName, dupRes.Status);

        client.UnregisterAll();
        using var greet3 = client.RegisterTool<GreetInput, GreetOutput>("greet", "问好", Handler);

        client.SetVisibility(AppVisibility.Hidden, false);
        client.Stop();
        var stopped = Assert.Throws<AppMcpException>(() => client.RegisterTool<GreetInput, GreetOutput>("after.stop", "x", Handler));
        Assert.Equal(AppMcpStatus.Stopped, stopped.Status);
    }

    [Fact]
    public void HandlesAfterClientDisposeReportStopped()
    {
        var client = AppMcpClient.Create(Options());
        var tool = client.RegisterTool("t", "d", (_, _) => Task.FromResult<object?>(null));
        client.Dispose();
        var e = Assert.Throws<AppMcpException>(() => tool.SetEnabled(false));
        Assert.Equal(AppMcpStatus.Stopped, e.Status);
        tool.Dispose(); // 不抛出
        client.Dispose(); // 幂等
    }

    [Fact]
    public void DispatcherDefaultsToCurrentContext()
    {
        var previous = SynchronizationContext.Current;
        var ctx = new SynchronizationContext();
        SynchronizationContext.SetSynchronizationContext(ctx);
        try
        {
            using var captured = AppMcpClient.Create(new AppMcpClientOptions { AppId = "dotnet-test", AppName = "x", HostUrl = "ws://127.0.0.1:1" });
            Assert.Same(ctx, captured.Dispatcher);
            using var explicitNull = AppMcpClient.Create(Options());
            Assert.Null(explicitNull.Dispatcher);
        }
        finally
        {
            SynchronizationContext.SetSynchronizationContext(previous);
        }
    }

    [Fact]
    public void RegistrationFailureReleasesHandler()
    {
        using var client = AppMcpClient.Create(Options());
        var weak = RegisterAndFail(client);
        for (var i = 0; i < 5 && weak.IsAlive; i++)
        {
            GC.Collect();
            GC.WaitForPendingFinalizers();
        }
        Assert.False(weak.IsAlive);
    }

    [System.Runtime.CompilerServices.MethodImpl(System.Runtime.CompilerServices.MethodImplOptions.NoInlining)]
    private static WeakReference RegisterAndFail(AppMcpClient client)
    {
        var payload = new object();
        var weak = new WeakReference(payload);
        Assert.Throws<AppMcpException>(() => client.RegisterTool("bad name!", "x", (_, _) => Task.FromResult<object?>(payload)));
        return weak;
    }
}
