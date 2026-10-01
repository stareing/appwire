using System.Collections.Concurrent;
using System.Diagnostics;
using System.Text.Json.Nodes;
using Xunit.Abstractions;

namespace AppMcp.Tests;

/// <summary>
/// 集成测试：启动 fake_host（crates/native/examples/fake_host.rs）→ 读取 LISTENING 地址 →
/// 用该地址启动客户端并注册工具 → 检查 fake_host 输出的调用结果。
/// fake_host 不存在（或无法构建）时测试直接返回并在输出中说明。
/// </summary>
public class IntegrationTests(ITestOutputHelper output)
{
    [Fact]
    public async Task InvokesToolsThroughFakeHost()
    {
        var fakeHost = FakeHost.Locate(output);
        if (fakeHost is null) return;

        using var host = FakeHost.Start(fakeHost,
            "--invoke", "greet", "--args", """{"name":"World"}""",
            "--invoke", "greet", "--args", "{}",
            "--invoke", "fail.custom", "--args", "{}",
            "--invoke", "throws", "--args", "{}",
            "--invoke", "raw.echo", "--args", """{"x":1}""",
            "--read", "app.info",
            "--timeout-ms", "20000");
        var addr = await host.ReadListeningAsync();

        // 单线程调度器：验证 handler 被切换到调度器线程上执行。
        using var ui = new SingleThreadSynchronizationContext();
        var handlerThreads = new ConcurrentBag<int>();

        var states = new ConcurrentQueue<ClientStatus>();
        await using var client = AppMcpClient.Create(new AppMcpClientOptions
        {
            AppId = "dotnet-it",
            AppName = ".NET IT",
            HostUrl = $"ws://{addr}",
            Dispatcher = ui,
        });
        client.StateChanged += (_, e) => states.Enqueue(e.State.Status);

        // 连接期间（handler 内）可查 Host 分配的连接 ID（fake_host 返回 "fake-<pid>"，spec/protocol.md 10.3）。
        var connectionIds = new ConcurrentBag<string?>();
        using var greet = client.RegisterTool<GreetInput, GreetOutput>("greet", "问好", async (input, ctx) =>
        {
            connectionIds.Add(client.ConnectionId);
            handlerThreads.Add(Environment.CurrentManagedThreadId);
            if (string.IsNullOrEmpty(input.Name)) throw new ToolCallException(ToolErrorKind.InvalidInput, "缺少 name 参数");
            await Task.Yield(); // 仍在调度器线程上继续
            handlerThreads.Add(Environment.CurrentManagedThreadId);
            ctx.AddStateHint("greetings");
            ctx.Progress(1, 2, "问候中");
            return new GreetOutput($"Hello, {input.Name}!");
        });
        using var custom = client.RegisterTool("fail.custom", "拒绝", (_, _) =>
            Task.FromException<object?>(new ToolCallException(ToolErrorKind.UserRejected, "用户拒绝")));
        using var throws = client.RegisterTool("throws", "抛异常", (_, _) =>
            throw new InvalidOperationException("出错了"));
        using var echo = client.RegisterTool("raw.echo", "回显", (args, _) =>
            Task.FromResult<object?>(new { echoed = args }));
        using var info = client.RegisterResource("app.info", "信息", _ => Task.FromResult<object?>(new { app = "dotnet", lang = "c#" }));

        client.Start();
        var lines = await host.WaitForExitAsync(TimeSpan.FromSeconds(30));
        foreach (var l in lines) output.WriteLine(l);
        Assert.Equal(0, host.ExitCode);

        var json = lines.Where(l => l.StartsWith('{')).Select(l => JsonNode.Parse(l)!.AsObject()).ToList();
        var tools = json.Single(j => (string?)j["type"] == "tools");
        Assert.Contains("greet", tools["tools"]!.AsArray().Select(t => (string?)t));

        var invokes = json.Where(j => (string?)j["type"] == "invoke").ToList();
        Assert.Equal(5, invokes.Count);

        var progress = Assert.Single(json, j => (string?)j["type"] == "progress");
        Assert.Equal(1.0, (double?)progress["progress"]);
        Assert.Equal(2.0, (double?)progress["total"]);
        Assert.Equal("问候中", (string?)progress["message"]);

        Assert.Equal("Hello, World!", (string?)invokes[0]["result"]!["data"]!["greeting"]);
        Assert.Equal("greetings", (string?)invokes[0]["result"]!["stateHints"]![0]);
        Assert.Equal("INVALID_INPUT", (string?)invokes[1]["error"]!["data"]!["kind"]);
        Assert.Equal("USER_REJECTED", (string?)invokes[2]["error"]!["data"]!["kind"]);
        Assert.Equal("HANDLER_ERROR", (string?)invokes[3]["error"]!["data"]!["kind"]);
        Assert.Contains("出错了", (string?)invokes[3]["error"]!["message"]);
        Assert.Equal(1, (int?)invokes[4]["result"]!["data"]!["echoed"]!["x"]);

        var read = json.Single(j => (string?)j["type"] == "read");
        Assert.Equal("c#", (string?)read["result"]!["contents"]!["lang"]);

        Assert.NotEmpty(handlerThreads);
        Assert.All(handlerThreads, id => Assert.Equal(ui.ThreadId, id));
        Assert.Contains(ClientStatus.Connected, states);
        Assert.NotEmpty(connectionIds);
        Assert.All(connectionIds, id => Assert.StartsWith("fake-", id));
    }

    /// <summary>带注解 + outputSchema 注册后 Host 收到的 ToolInfo；结构化结果（pending 等）与普通返回值（回归）。</summary>
    [Fact]
    public async Task ToolOptionsAndStructuredResultReachHost()
    {
        var fakeHost = FakeHost.Locate(output);
        if (fakeHost is null) return;

        using var host = FakeHost.Start(fakeHost,
            "--tool-info",
            "--invoke", "order.submit", "--args", "{}",
            "--invoke", "order.typed", "--args", """{"name":"x"}""",
            "--invoke", "plain", "--args", "{}",
            "--timeout-ms", "20000");
        var addr = await host.ReadListeningAsync();
        await using var client = AppMcpClient.Create(new AppMcpClientOptions
        {
            AppId = "dotnet-result",
            AppName = ".NET Result",
            HostUrl = $"ws://{addr}",
            Dispatcher = null,
        });
        using var submit = client.RegisterTool("order.submit", "下单", (_, ctx) =>
        {
            ctx.AddStateHint("cart");
            return Task.FromResult<object?>(new ToolResult(new { orderId = "o1" })
            {
                Status = ToolResultStatus.Pending,
                StateResource = "order.state",
                Summary = "已提交，等待用户在 App 内付款",
                Annotations = new ContentAnnotations { Priority = 0.5, Audience = [ContentAudience.User] },
                StateHints = ["orders"],
            });
        }, new ToolOptions
        {
            Annotations = new ToolAnnotations { IdempotentHint = false, OpenWorldHint = true },
            OutputSchemaJson = """{"type":"object","properties":{"orderId":{"type":"string"}}}""",
        });
        using var typed = client.RegisterTool<GreetInput, ToolResult>("order.typed", "类型化", (input, _) =>
            Task.FromResult(new ToolResult { Status = ToolResultStatus.Noop, Summary = $"{input.Name} 已是目标状态" }));
        using var plain = client.RegisterTool("plain", "普通", (_, _) => Task.FromResult<object?>(new { ok = true }),
            new ToolOptions { Risk = ToolRisk.Read });

        client.Start();
        var lines = await host.WaitForExitAsync(TimeSpan.FromSeconds(30));
        foreach (var l in lines) output.WriteLine(l);
        Assert.Equal(0, host.ExitCode);

        var json = lines.Where(l => l.StartsWith('{')).Select(l => JsonNode.Parse(l)!.AsObject()).ToList();
        var info = json.Single(j => (string?)j["type"] == "tools")["toolInfo"]!;
        Assert.True(JsonNode.DeepEquals(
            JsonNode.Parse("""
                {"risk":"write","annotations":{"idempotentHint":false,"openWorldHint":true},
                 "outputSchema":{"type":"object","properties":{"orderId":{"type":"string"}}}}
                """),
            info["order.submit"]), info.ToJsonString());
        Assert.True(JsonNode.DeepEquals(JsonNode.Parse("""{"risk":"read"}"""), info["plain"]), info.ToJsonString());

        var invokes = json.Where(j => (string?)j["type"] == "invoke").ToList();
        Assert.Equal(3, invokes.Count);
        Assert.True(JsonNode.DeepEquals(
            JsonNode.Parse("""
                {"data":{"orderId":"o1"},"stateHints":["cart","orders"],"status":"pending","stateResource":"order.state",
                 "summary":"已提交，等待用户在 App 内付款","annotations":{"audience":["user"],"priority":0.5}}
                """),
            invokes[0]["result"]), invokes[0].ToJsonString());
        Assert.True(JsonNode.DeepEquals(
            JsonNode.Parse("""{"data":null,"status":"noop","summary":"x 已是目标状态"}"""),
            invokes[1]["result"]), invokes[1].ToJsonString());
        Assert.True(JsonNode.DeepEquals(JsonNode.Parse("""{"data":{"ok":true}}"""), invokes[2]["result"]), invokes[2].ToJsonString());
    }
}

/// <summary>在专用线程上执行所有回调的 SynchronizationContext（模拟 UI 线程）。</summary>
internal sealed class SingleThreadSynchronizationContext : SynchronizationContext, IDisposable
{
    private readonly BlockingCollection<(SendOrPostCallback, object?)> _queue = new();
    private readonly Thread _thread;

    public SingleThreadSynchronizationContext()
    {
        _thread = new Thread(() =>
        {
            SetSynchronizationContext(this);
            foreach (var (cb, state) in _queue.GetConsumingEnumerable()) cb(state);
        }) { IsBackground = true, Name = "fake-ui" };
        _thread.Start();
    }

    public int ThreadId => _thread.ManagedThreadId;

    public override void Post(SendOrPostCallback d, object? state)
    {
        try { _queue.Add((d, state)); } catch (InvalidOperationException) { }
    }

    public override void Send(SendOrPostCallback d, object? state) => throw new NotSupportedException();

    public void Dispose() => _queue.CompleteAdding();
}

internal sealed class FakeHost : IDisposable
{
    private readonly Process _process;
    private readonly List<string> _lines = new();
    private readonly TaskCompletionSource<string> _listening = new(TaskCreationOptions.RunContinuationsAsynchronously);
    private readonly TaskCompletionSource _stdoutClosed = new(TaskCreationOptions.RunContinuationsAsynchronously);

    private FakeHost(Process process)
    {
        _process = process;
        _process.OutputDataReceived += (_, e) =>
        {
            if (e.Data is null)
            {
                _stdoutClosed.TrySetResult();
                return;
            }
            lock (_lines) _lines.Add(e.Data);
            if (e.Data.StartsWith("LISTENING ", StringComparison.Ordinal)) _listening.TrySetResult(e.Data["LISTENING ".Length..].Trim());
        };
        _process.ErrorDataReceived += (_, e) =>
        {
            if (e.Data is not null) lock (_lines) _lines.Add("[stderr] " + e.Data);
        };
        _process.BeginOutputReadLine();
        _process.BeginErrorReadLine();
    }

    public int ExitCode => _process.ExitCode;

    public static string? Locate(ITestOutputHelper output)
    {
        var env = Environment.GetEnvironmentVariable("FAKE_HOST");
        if (!string.IsNullOrEmpty(env) && File.Exists(env)) return env;

        var repo = FindRepoRoot();
        if (repo is null || !File.Exists(Path.Combine(repo, "crates/native/examples/fake_host.rs")))
        {
            output.WriteLine("SKIP：fake_host 尚不存在");
            return null;
        }
        var targetDir = Environment.GetEnvironmentVariable("CARGO_TARGET_DIR") ?? Path.Combine(repo, "target");
        var exe = Path.Combine(targetDir, "debug", "examples", OperatingSystem.IsWindows() ? "fake_host.exe" : "fake_host");

        // 始终构建一次（增量，已是最新时很快）。
        var psi = new ProcessStartInfo("cargo", "build -q -p app-mcp-native --example fake_host")
        {
            WorkingDirectory = repo,
            RedirectStandardError = true,
            UseShellExecute = false,
        };
        psi.Environment["CARGO_TARGET_DIR"] = targetDir;
        try
        {
            using var build = Process.Start(psi)!;
            var err = build.StandardError.ReadToEnd();
            build.WaitForExit();
            if (build.ExitCode != 0) output.WriteLine("cargo build fake_host 失败：" + err);
        }
        catch (Exception e)
        {
            output.WriteLine("无法运行 cargo：" + e.Message);
        }
        if (File.Exists(exe)) return exe;
        output.WriteLine("SKIP：找不到 " + exe);
        return null;
    }

    private static string? FindRepoRoot()
    {
        var dir = new DirectoryInfo(AppContext.BaseDirectory);
        while (dir is not null)
        {
            if (File.Exists(Path.Combine(dir.FullName, "Cargo.toml")) && Directory.Exists(Path.Combine(dir.FullName, "crates", "native")))
                return dir.FullName;
            dir = dir.Parent;
        }
        return null;
    }

    public static FakeHost Start(string exe, params string[] args)
    {
        var psi = new ProcessStartInfo(exe)
        {
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            UseShellExecute = false,
            // fake_host 输出 UTF-8；Windows 默认按控制台代码页（如 936）解码会损坏中文，导致 JSON 解析失败。
            StandardOutputEncoding = System.Text.Encoding.UTF8,
            StandardErrorEncoding = System.Text.Encoding.UTF8,
        };
        psi.ArgumentList.Add("--addr");
        psi.ArgumentList.Add("127.0.0.1:0");
        foreach (var a in args) psi.ArgumentList.Add(a);
        return new FakeHost(Process.Start(psi)!);
    }

    public async Task<string> ReadListeningAsync()
    {
        var done = await Task.WhenAny(_listening.Task, Task.Delay(TimeSpan.FromSeconds(10)));
        if (done != _listening.Task) throw new TimeoutException("fake_host 没有输出 LISTENING");
        return await _listening.Task;
    }

    /// <summary>等待第一行满足条件的输出（已输出的行也算）。</summary>
    public async Task<string> WaitForLineAsync(Func<string, bool> predicate, TimeSpan timeout)
    {
        var deadline = DateTime.UtcNow + timeout;
        while (DateTime.UtcNow < deadline)
        {
            lock (_lines)
            {
                var hit = _lines.FirstOrDefault(predicate);
                if (hit is not null) return hit;
            }
            if (_process.HasExited && _stdoutClosed.Task.IsCompleted) break;
            await Task.Delay(20);
        }
        lock (_lines) throw new TimeoutException("fake_host 没有输出期望的行：\n" + string.Join("\n", _lines));
    }

    public async Task<IReadOnlyList<string>> WaitForExitAsync(TimeSpan timeout)
    {
        using var cts = new CancellationTokenSource(timeout);
        await _process.WaitForExitAsync(cts.Token);
        await Task.WhenAny(_stdoutClosed.Task, Task.Delay(2000));
        lock (_lines) return _lines.ToArray();
    }

    public void Dispose()
    {
        try
        {
            if (!_process.HasExited) _process.Kill();
        }
        catch
        {
        }
        _process.Dispose();
    }
}
