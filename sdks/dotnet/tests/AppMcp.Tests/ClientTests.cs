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
    public void ErrorKindStrings(ToolErrorKind kind, string expected)
    {
        Assert.Equal(expected, kind.ToProtocolString());
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
