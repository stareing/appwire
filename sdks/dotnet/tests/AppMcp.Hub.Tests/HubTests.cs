using System.Collections.Concurrent;
using System.Text.Json;
using System.Text.Json.Nodes;
using AppMcp.Hub.Native;

namespace AppMcp.Hub.Tests;

public class HubBasicTests
{
    [Fact]
    public void NativeLibraryLoads()
    {
        Assert.False(string.IsNullOrEmpty(AppMcpHub.Version));
    }

    [Fact]
    public void ApiVersionMatchesHeader()
    {
        var dir = new DirectoryInfo(AppContext.BaseDirectory);
        while (dir is not null && !File.Exists(HeaderPath(dir.FullName))) dir = dir.Parent;
        Assert.NotNull(dir);
        var header = File.ReadAllText(HeaderPath(dir!.FullName));
        Assert.Contains($"#define AM_HUB_API_VERSION {HubNativeMethods.ApiVersion}", header);
        // 枚举值与头文件一致
        Assert.Contains("AM_HUB_FORMAT_ANTHROPIC = 3", header);
        Assert.Equal(3, (int)ToolFormat.Anthropic);
        Assert.Contains("AM_HUB_ERR_PANIC = 9", header);
        Assert.Equal(9, (int)HubStatus.Panic);
    }

    private static string HeaderPath(string root) => Path.Combine(root, "bindings", "hub-c", "include", "app_mcp_hub.h");

    [Fact]
    public void OptionsSerializeToConfigJson()
    {
        var o = new HubOptions
        {
            Listen = "127.0.0.1:0",
            IpcEndpoint = "unix:/run/x/hub.sock",
            RequireApprovalAtOrAbove = HubRisk.OsSensitive,
            ApprovalTimeout = TimeSpan.FromSeconds(3),
            ResponseTimeout = TimeSpan.FromMilliseconds(1500),
            WorkerThreads = 1,
        };
        o.AllowOrigins.Add("http://localhost:*");
        o.Upstreams["fs"] = new UpstreamOptions { Command = "mcp-fs", Args = ["--root", "/tmp"] };
        var json = JsonNode.Parse(o.ToConfigJson())!.AsObject();
        Assert.Equal("127.0.0.1:0", (string?)json["listen"]);
        Assert.False(json.ContainsKey("mcpHttp"));
        var extra = JsonNode.Parse(new HubOptions { McpHttp = true, RunDir = "/tmp/r" }.ToConfigJson())!.AsObject();
        Assert.True((bool?)extra["mcpHttp"]);
        Assert.Equal("/tmp/r", (string?)extra["runDir"]);
        Assert.Equal("unix:/run/x/hub.sock", (string?)json["ipcEndpoint"]);
        Assert.Equal("os-sensitive", (string?)json["approval"]!["requireAtOrAbove"]);
        Assert.Equal(3000, (int?)json["approval"]!["timeout"]);
        Assert.Equal(1500, (int?)json["responseTimeoutMs"]);
        Assert.Equal("mcp-fs", (string?)json["upstreams"]!["fs"]!["command"]);
        Assert.False(json.ContainsKey("manifests"));

        var life = JsonNode.Parse(new HubOptions
        {
            DisableIpc = true,
            Listen = "127.0.0.1:0",
            LeaseTtl = TimeSpan.Zero,
            WakeTimeout = TimeSpan.FromSeconds(2),
            WakeTokenTtl = TimeSpan.FromSeconds(3),
            DormantTtl = TimeSpan.FromMinutes(1),
            DormantReplacedByNewInstance = false,
            WakeFromLaunch = true,
        }.ToConfigJson())!.AsObject();
        Assert.Equal(0, (int?)life["leaseTtlMs"]);
        Assert.Equal(2000, (int?)life["wakeTimeoutMs"]);
        Assert.Equal(3000, (int?)life["wakeTokenTtlMs"]);
        Assert.Equal(60000, (int?)life["dormantTtlMs"]);
        Assert.False((bool?)life["dormantReplacedByNewInstance"]);
        Assert.True((bool?)life["wakeFromLaunch"]);
        using (var h = AppMcpHub.Start(life.ToJsonString(), null)) { }

        var exposure = JsonNode.Parse(new HubOptions
        {
            DisableIpc = true,
            Listen = "127.0.0.1:0",
            ToolExposure = ToolExposure.Progressive,
            ToolExposureThreshold = 5,
            Waker = WakerOptions.Exec("node", "wake.mjs"),
        }.ToConfigJson())!.AsObject();
        Assert.Equal("progressive", (string?)exposure["toolExposure"]);
        Assert.Equal(5, (int?)exposure["toolExposureThreshold"]);
        Assert.Equal("wake.mjs", (string?)exposure["waker"]!["exec"]![1]);
        Assert.Equal("none", (string?)JsonNode.Parse(new HubOptions { Waker = WakerOptions.None }.ToConfigJson())!["waker"]);
        using (var h = AppMcpHub.Start(exposure.ToJsonString(), null))
        {
            // 渐进暴露：没有展开的 App 时只有内置工具（含 apps.tools）
            var names = h.ListTools(new ToolFilter { Session = "c1" }).Select(t => t.Name).ToArray();
            Assert.Equal(["apps.list", "apps.select", "apps.overview", "apps.tools"], names);
        }

        var disabled = JsonNode.Parse(new HubOptions { DisableListen = true, DisableIpc = true }.ToConfigJson())!.AsObject();
        Assert.True(disabled.ContainsKey("listen"));
        Assert.Null(disabled["listen"]);
        Assert.True(disabled.ContainsKey("ipcEndpoint"));
        Assert.Null(disabled["ipcEndpoint"]);
        Assert.False(JsonNode.Parse(new HubOptions().ToConfigJson())!.AsObject().ContainsKey("ipcEndpoint"));
    }

    [Fact]
    public void StartErrorsMapToStatus()
    {
        var e = Assert.Throws<HubException>(() => AppMcpHub.Start("""{"bogus":1}""", null));
        Assert.Equal(HubStatus.InvalidJson, e.Status);
        Assert.False(string.IsNullOrEmpty(e.Message));

        var o = new HubOptions { DisableListen = true, DisableIpc = true, Dispatcher = null };
        o.Manifests.Add(JsonNode.Parse("""{"appId":""}""")!);
        Assert.Equal(HubStatus.InvalidConfig, Assert.Throws<HubException>(() => AppMcpHub.Start(o)).Status);
    }

    [Fact]
    public async Task QueriesWithoutApps()
    {
        using var hub = AppMcpHub.Start(new HubOptions { DisableListen = true, DisableIpc = true, Dispatcher = null });
        Assert.Null(hub.ListenAddress);
        Assert.Null(hub.IpcEndpoint);
        Assert.Equal(0, hub.GetApps().GetArrayLength());
        Assert.Equal(0, hub.GetResources().GetArrayLength());
        Assert.Null(hub.GetOverview("nope"));

        var status = hub.Status();
        Assert.Equal("app-mcp", status.Service);
        Assert.Equal((uint)Environment.ProcessId, status.Pid);
        Assert.Null(status.Listen);
        Assert.Null(status.IpcEndpoint);
        Assert.True(status.StartedAtMs > 0);
        Assert.False(status.Auth.TokenConfigured);
        Assert.Equal(0, status.McpSessions);
        Assert.Empty(status.Apps);
        Assert.Empty(status.Reports);
        Assert.Equal("app-mcp", hub.GetStatus().GetProperty("service").GetString());

        // 内置工具 apps.list 等
        var tools = hub.GetTools();
        Assert.Contains(tools.EnumerateArray(), t => t.GetProperty("name").GetString() == "apps.list");
        Assert.Equal(0, hub.GetTools(new ToolFilter { IncludeBuiltin = false }).GetArrayLength());
        var exported = hub.ExportTools(ToolFormat.OpenAiChat);
        Assert.Equal(JsonValueKind.Array, exported.ValueKind);

        var list = await hub.CallAsync("apps.list");
        Assert.True(list.IsSuccess, list.Json.GetRawText());
        Assert.False(string.IsNullOrEmpty(list.CallId));

        var missing = await hub.CallAsync("ghost.tool");
        Assert.Equal("TOOL_NOT_FOUND", missing.Error?.Kind);

        var read = await Assert.ThrowsAsync<HubCallException>(() => hub.ReadResourceAsync("app-mcp://ghost/x"));
        Assert.False(string.IsNullOrEmpty(read.Error.Kind));

        hub.Shutdown();
        hub.Shutdown(); // 幂等
        Assert.Equal(HubStatus.Stopped, Assert.Throws<HubException>(() => hub.GetApps()).Status);
        Assert.Equal(HubStatus.Stopped, Assert.Throws<HubException>(() => hub.Status()).Status);
        Assert.Equal(HubStatus.Stopped, (await Assert.ThrowsAsync<HubException>(() => hub.CallAsync("apps.list"))).Status);
    }

    [Fact]
    public void UseAfterDisposeThrows()
    {
        var hub = AppMcpHub.Start(new HubOptions { DisableListen = true, DisableIpc = true, Dispatcher = null });
        hub.Dispose();
        hub.Dispose();
        Assert.Throws<ObjectDisposedException>(() => hub.GetApps());
        Assert.Throws<ObjectDisposedException>(() => hub.ApprovalHandler = (_, _) => Task.FromResult(true));
    }
}

/// <summary>
/// 集成：嵌入式 Hub + App 端 C# SDK（AppMcp，libapp_mcp）在同一进程内通过 WebSocket 连接，
/// 覆盖事件、列工具、调用、审批、格式导出与分派、资源读取。
/// </summary>
public class HubIntegrationTests
{
    private static readonly TimeSpan Wait = TimeSpan.FromSeconds(15);

    [Fact]
    public async Task AppRoundTripEventsApprovalAndDispatch()
    {
        using var ui = new SingleThreadSynchronizationContext();
        await using var hub = AppMcpHub.Start(new HubOptions
        {
            DisableIpc = true,
            Listen = "127.0.0.1:0",
            RequireApprovalAtOrAbove = HubRisk.Destructive,
            ApprovalTimeout = TimeSpan.FromSeconds(10),
            Dispatcher = ui,
        });
        var addr = hub.ListenAddress;
        Assert.False(string.IsNullOrEmpty(addr));

        var events = new ConcurrentQueue<HubEventArgs>();
        var eventThreads = new ConcurrentBag<int>();
        var connected = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        hub.Event += (_, e) =>
        {
            eventThreads.Add(Environment.CurrentManagedThreadId);
            events.Enqueue(e);
            if (e.Type == "appConnected" && e.AppId == "notes") connected.TrySetResult();
        };

        // 审批：按队列中的决定放行 / 拒绝；队列为空时一直挂起（用于测试取消）。
        var decisions = new ConcurrentQueue<bool>();
        var approvals = new ConcurrentQueue<ApprovalRequest>();
        var approvalThreads = new ConcurrentBag<int>();
        hub.ApprovalHandler = async (req, ct) =>
        {
            approvalThreads.Add(Environment.CurrentManagedThreadId);
            approvals.Enqueue(req);
            if (decisions.TryDequeue(out var d)) return d;
            await Task.Delay(Timeout.Infinite, ct);
            return false;
        };

        await using var app = AppMcp.AppMcpClient.Create(new AppMcp.AppMcpClientOptions
        {
            AppId = "notes",
            AppName = "笔记",
            InstanceId = "n1",
            HostUrl = $"ws://{addr}/app",
            Overview = new AppMcp.AppOverview("笔记 App：可添加与删除笔记"),
            Dispatcher = null,
        });
        var notes = new ConcurrentQueue<string>();
        using var add = app.RegisterTool("add", "添加笔记", (args, ctx) =>
        {
            var text = args.GetProperty("text").GetString() ?? string.Empty;
            notes.Enqueue(text);
            ctx.AddStateHint("notes.list");
            return Task.FromResult<object?>(new { echo = new { text } });
        }, new AppMcp.ToolOptions
        {
            InputSchemaJson = """{"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}""",
        });
        using var del = app.RegisterTool("delete", "删除全部笔记", (_, _) =>
        {
            notes.Clear();
            return Task.FromResult<object?>(new { deleted = true });
        }, new AppMcp.ToolOptions { Risk = AppMcp.ToolRisk.Destructive });
        using var list = app.RegisterResource("list", "笔记列表", _ => Task.FromResult<object?>(notes.ToArray()));
        app.Start();

        await connected.Task.WaitAsync(Wait);
        await WaitUntil(() => hub.GetTools(new ToolFilter { Apps = ["notes"], OnlyAvailable = true, IncludeBuiltin = false })
            .GetArrayLength() == 2, "工具未同步");
        Assert.All(eventThreads, id => Assert.Equal(ui.ThreadId, id));

        var apps = hub.GetApps();
        Assert.Equal("notes", apps[0].GetProperty("appId").GetString());

        // 运行状态：实例带连接 ID，与 ListApps 一致
        var status = hub.Status();
        Assert.Equal(addr, status.Listen);
        var notesStatus = Assert.Single(status.Apps, a => a.AppId == "notes");
        Assert.Equal("connected", notesStatus.State);
        var inst = Assert.Single(notesStatus.Instances);
        Assert.Equal("n1", inst.InstanceId);
        Assert.Equal("connected", inst.State);
        Assert.Matches("^[0-9a-f]+-[0-9]+$", inst.ConnectionId);
        Assert.Equal(inst.ConnectionId, hub.ListApps().Single(a => a.AppId == "notes").Instances[0].ConnectionId);

        // 普通调用：成功 + stateHints + 首次附带总览
        var ok = await hub.CallAsync(new CallRequest("notes.add", new { text = "买牛奶" }) { Session = "s1" });
        Assert.True(ok.IsSuccess, ok.Json.GetRawText());
        Assert.Equal("买牛奶", ok.Data!.Value.GetProperty("echo").GetProperty("text").GetString());
        Assert.Equal(["notes.list"], ok.StateHints);
        Assert.Equal("n1", ok.InstanceId);
        Assert.Equal("notes", ok.Overview?.GetProperty("appId").GetString());
        Assert.NotNull(hub.GetOverview("notes"));

        // 参数不合法
        var bad = await hub.CallAsync("notes.add", new { });
        Assert.Equal("INVALID_INPUT", bad.Error?.Kind);

        // 审批：拒绝 → USER_REJECTED；同意 → 成功
        decisions.Enqueue(false);
        Assert.Equal("USER_REJECTED", (await hub.CallAsync("notes.delete")).Error?.Kind);
        Assert.True(approvals.TryPeek(out var req));
        Assert.Equal("delete", req.Tool);
        Assert.Equal("destructive", req.Risk);
        Assert.Equal("notes", req.AppId);
        Assert.All(approvalThreads, id => Assert.Equal(ui.ThreadId, id));

        decisions.Enqueue(true);
        var deleted = await hub.CallAsync("notes.delete");
        Assert.True(deleted.IsSuccess, deleted.Json.GetRawText());

        // 审批等待中取消（CancellationToken → am_hub_cancel_call）
        using var cts = new CancellationTokenSource();
        var pending = hub.CallAsync("notes.delete", null, cts.Token);
        await WaitUntil(() => approvals.Count == 3, "未收到第三次审批请求");
        cts.Cancel();
        Assert.Equal("CANCELLED", (await pending.WaitAsync(Wait)).Error?.Kind);

        // 格式导出 + dispatch（Anthropic）
        var exported = hub.ExportTools(ToolFormat.Anthropic, new ToolFilter { Apps = ["notes"], IncludeBuiltin = false });
        Assert.Contains(exported.EnumerateArray(), t => t.GetProperty("name").GetString() == "notes__add");
        var result = await hub.DispatchAsync(ToolFormat.Anthropic,
            JsonNode.Parse("""{"type":"tool_use","id":"toolu_9","name":"notes__add","input":{"text":"x"}}""")!);
        Assert.Equal("toolu_9", result.GetProperty("tool_use_id").GetString());
        Assert.Contains("echo", result.GetProperty("content").GetRawText());

        // 资源：读取 + 订阅
        var contents = await hub.ReadResourceAsync("app-mcp://notes/list");
        Assert.Contains("x", contents.Text);
        hub.Subscribe("app-mcp://notes/list");
        hub.Unsubscribe("app-mcp://notes/list");

        // 实例选择与会话重置
        hub.SelectInstance("notes", "n1");
        Assert.Equal("n1", hub.GetApps()[0].GetProperty("selectedInstance").GetString());
        hub.SelectInstance("notes", null);
        hub.ResetSession("s1");
        var again = await hub.CallAsync(new CallRequest("notes.add", new { text = "再来" }) { Session = "s1" });
        Assert.NotNull(again.Overview);

        app.Stop();
    }

    [Fact]
    public async Task PairingHandlerAcceptsAndRejects()
    {
        await using var hub = AppMcpHub.Start(new HubOptions { Listen = "127.0.0.1:0", DisableIpc = true, Dispatcher = null });
        var requests = new ConcurrentQueue<PairingRequest>();
        hub.PairingHandler = (req, _) =>
        {
            requests.Enqueue(req);
            return Task.FromResult(req.AppId == "good");
        };
        var url = $"ws://{hub.ListenAddress}/app";

        await using var good = AppMcp.AppMcpClient.Create(new AppMcp.AppMcpClientOptions { AppId = "good", AppName = "Good", HostUrl = url, Dispatcher = null });
        good.Start();
        await WaitUntil(() => hub.GetApps().EnumerateArray().Any(a =>
            a.GetProperty("appId").GetString() == "good" && a.GetProperty("connected").GetBoolean()), "配对后未连接");

        await using var bad = AppMcp.AppMcpClient.Create(new AppMcp.AppMcpClientOptions { AppId = "bad", AppName = "Bad", HostUrl = url, Dispatcher = null });
        bad.Start();
        await WaitUntil(() => bad.State.Status == AppMcp.ClientStatus.Rejected, "未被拒绝");

        Assert.Contains(requests, r => r.AppId == "good" && r.ClientKind == "native");
        Assert.Contains(requests, r => r.AppId == "bad");

        // 清除后不再调用 handler
        hub.PairingHandler = null;
        Assert.Null(hub.PairingHandler);
        good.Stop();
        bad.Stop();
    }

    [Fact]
    public async Task DormantAppWokenByCustomWaker()
    {
        await using var hub = AppMcpHub.Start(new HubOptions
        {
            DisableIpc = true,
            Listen = "127.0.0.1:0",
            ListChangedDebounce = TimeSpan.FromMilliseconds(20),
            WakeTimeout = TimeSpan.FromSeconds(10),
            LeaseTtl = TimeSpan.Zero,
            Dispatcher = null,
        });
        var dormant = new TaskCompletionSource<HubEventArgs>(TaskCreationOptions.RunContinuationsAsynchronously);
        var waking = new TaskCompletionSource<HubEventArgs>(TaskCreationOptions.RunContinuationsAsynchronously);
        hub.Event += (_, e) =>
        {
            if (e.Type == HubEventTypes.AppDormant && e.AppId == "sleepy") dormant.TrySetResult(e);
            if (e.Type == HubEventTypes.AppWaking && e.AppId == "sleepy") waking.TrySetResult(e);
        };

        await using var app = AppMcp.AppMcpClient.Create(new AppMcp.AppMcpClientOptions
        {
            AppId = "sleepy",
            AppName = "会睡觉的 App",
            InstanceId = "s1",
            HostUrl = $"ws://{hub.ListenAddress}/app",
            Dispatcher = null,
            Lifecycle = new AppMcp.LifecycleOptions
            {
                Mode = AppMcp.LifecycleMode.Idle,
                IdleTimeout = TimeSpan.FromMilliseconds(300),
                Wake = new AppMcp.WakeDescriptor(AppMcp.WakeKind.AndroidIntent, "dev.example/.WakeReceiver", true),
            },
        });
        using var ping = app.RegisterTool("ping", "回显", (args, _) =>
            Task.FromResult<object?>(new { echo = args.GetRawText() }));

        // Waker：厂商在这里发送广播；测试中直接让同进程 App 处理激活参数。
        var wakes = new ConcurrentQueue<WakeRequest>();
        hub.Waker = (req, _) =>
        {
            wakes.Enqueue(req);
            if (!app.HandleWake(req.ActivationArg)) throw new WakeFailedException("LAUNCH_FAILED", "不认识的激活参数");
            return Task.CompletedTask;
        };
        app.Start();

        var ev = await dormant.Task.WaitAsync(Wait);
        Assert.Equal("s1", ev.InstanceId);
        var info = hub.ListApps().Single(a => a.AppId == "sleepy");
        Assert.False(info.Connected);
        Assert.True(info.IsDormant);
        Assert.Equal("s1", info.DormantInstances[0].InstanceId);
        var tool = hub.ListTools(new ToolFilter { Apps = ["sleepy"], IncludeBuiltin = false }).Single();
        Assert.Equal(HubAvailability.Dormant, tool.Availability);
        Assert.Empty(hub.ListTools(new ToolFilter { Apps = ["sleepy"], IncludeBuiltin = false, OnlyAvailable = true }));

        var r = await hub.CallAsync("sleepy.ping", new { x = 1 }).WaitAsync(Wait);
        Assert.True(r.IsSuccess, r.Json.GetRawText());
        Assert.Equal("s1", r.InstanceId);
        Assert.True(wakes.TryDequeue(out var w));
        Assert.Equal("sleepy", w.AppId);
        Assert.Equal("s1", w.InstanceId);
        Assert.Equal("android-intent", w.Descriptor.Kind);
        Assert.Equal("dev.example/.WakeReceiver", w.Descriptor.Target);
        Assert.Equal($"app-mcp-wake:{w.Token}", w.ActivationArg);
        Assert.Equal("s1", (await waking.Task.WaitAsync(Wait)).InstanceId);

        // 失败：WakeFailedException 的类别透传；清除后恢复默认实现
        await dormant.Task; // 已休眠过一次
        var dormant2 = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        hub.Event += (_, e) => { if (e.Type == HubEventTypes.AppDormant) dormant2.TrySetResult(); };
        await dormant2.Task.WaitAsync(Wait);
        hub.Waker = (_, _) => throw new WakeFailedException("APP_NOT_INSTALLED", "没装");
        var failed = await hub.CallAsync("sleepy.ping").WaitAsync(Wait);
        Assert.Equal("APP_NOT_INSTALLED", failed.Error?.Kind);
        Assert.Contains("没装", failed.Error?.Message);
        hub.Waker = null;
        Assert.Null(hub.Waker);
        app.Stop();
    }

    [Fact]
    public async Task NativeAppOverIpcReportsPid()
    {
        var dir = Path.Combine(Path.GetTempPath(), $"app-mcp-cs-ipc-{Environment.ProcessId}");
        var endpoint = OperatingSystem.IsWindows()
            ? $@"pipe:\\.\pipe\app-mcp-cs-test-{Environment.ProcessId}"
            : $"unix:{Path.Combine(dir, "run", "hub.sock")}";
        try
        {
            await using var hub = AppMcpHub.Start(new HubOptions { DisableListen = true, IpcEndpoint = endpoint, Dispatcher = null });
            Assert.Equal(endpoint, hub.IpcEndpoint);
            await using var app = AppMcp.AppMcpClient.Create(new AppMcp.AppMcpClientOptions
            {
                AppId = "notes",
                AppName = "笔记",
                HostUrl = hub.IpcEndpoint,
                Dispatcher = null,
            });
            using var add = app.RegisterTool("add", "添加笔记", (_, _) => Task.FromResult<object?>(new { ok = true }));
            app.Start();
            await WaitUntil(() => hub.GetApps().EnumerateArray()
                .Any(a => a.GetProperty("appId").GetString() == "notes" && a.GetProperty("connected").GetBoolean()), "App 未经 IPC 连上");
            var instance = hub.GetApps().EnumerateArray().First(a => a.GetProperty("appId").GetString() == "notes")
                .GetProperty("instances")[0];
            Assert.Equal(Environment.ProcessId, instance.GetProperty("pid").GetInt32());

            // IPC（Windows 命名管道 / Unix 套接字）上同样分配连接 ID：App 端与 Hub 运行状态一致
            await WaitUntil(() => app.ConnectionId is not null, "App 未取得连接 ID");
            var status = hub.Status();
            Assert.Equal(endpoint, status.IpcEndpoint);
            var inst = Assert.Single(Assert.Single(status.Apps, a => a.AppId == "notes").Instances);
            Assert.Equal(app.ConnectionId, inst.ConnectionId);
            Assert.Null(app.State.Code);
        }
        finally
        {
            if (Directory.Exists(dir)) Directory.Delete(dir, true);
        }
    }

    private static async Task WaitUntil(Func<bool> condition, string message)
    {
        var deadline = DateTime.UtcNow + Wait;
        while (!condition())
        {
            if (DateTime.UtcNow > deadline) throw new TimeoutException(message);
            await Task.Delay(20);
        }
    }
}

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
