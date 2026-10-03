using System.Collections.Concurrent;
using System.Text.Json;
using System.Text.Json.Nodes;
using AppMcp.Hub.Native;

namespace AppMcp.Hub.Tests;

public class HubBasicTests
{
    [Fact]
    public void CallOutcomeParsesDurationAndWoke()
    {
        // spec/hub-api.md 3.15：durationMs / woke 只增字段；旧 Hub 缺省时为 0 / false。
        using var full = JsonDocument.Parse(
            """{"callId":"c1","result":{"ok":{"x":1}},"stateHints":[],"routedTo":"shop.cart.summary","durationMs":1234,"woke":true}""");
        var o = new CallOutcome(full.RootElement);
        Assert.Equal((1234L, true, "shop.cart.summary"), (o.DurationMs, o.Woke, o.RoutedTo));
        using var old = JsonDocument.Parse("""{"callId":"c2","result":{"error":{"kind":"TOOL_NOT_FOUND","message":"x"}}}""");
        var legacy = new CallOutcome(old.RootElement);
        Assert.Equal((0L, false, (string?)null), (legacy.DurationMs, legacy.Woke, legacy.RoutedTo));
        Assert.Equal("TOOL_NOT_FOUND", legacy.Error?.Kind);
    }

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
        Assert.Contains("AM_HUB_ERR_UNSUPPORTED = 10", header);
        Assert.Equal(10, (int)HubStatus.Unsupported);
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
        var extra = JsonNode.Parse(new HubOptions { McpHttp = true, RunDir = "/tmp/r", StateDir = "/tmp/s" }.ToConfigJson())!.AsObject();
        Assert.True((bool?)extra["mcpHttp"]);
        Assert.Equal("/tmp/r", (string?)extra["runDir"]);
        Assert.Equal("/tmp/s", (string?)extra["stateDir"]);
        Assert.False(json.ContainsKey("stateDir"));
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
            NavigateTimeout = TimeSpan.FromMilliseconds(800),
            WakeTokenTtl = TimeSpan.FromSeconds(3),
            DormantTtl = TimeSpan.FromMinutes(1),
            DormantReplacedByNewInstance = false,
            WakeFromLaunch = true,
        }.ToConfigJson())!.AsObject();
        Assert.Equal(0, (int?)life["leaseTtlMs"]);
        Assert.Equal(2000, (int?)life["wakeTimeoutMs"]);
        Assert.Equal(800, (int?)life["navigateTimeoutMs"]);
        Assert.False(json.ContainsKey("navigateTimeoutMs"));
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

        // 功耗（4e）：唤醒速率上限、旧心跳、自适应租约
        var power = JsonNode.Parse(new HubOptions
        {
            DisableIpc = true,
            DisableListen = true,
            WakeRateLimit = 0,
            LegacyHeartbeat = true,
            Lease = new LeaseOptions { Adaptive = false, Window = 4, Max = TimeSpan.FromSeconds(30), IdleRevoke = TimeSpan.Zero },
        }.ToConfigJson())!.AsObject();
        Assert.Equal(0, (int?)power["wakeRateLimit"]);
        Assert.True((bool?)power["legacyHeartbeat"]);
        Assert.False((bool?)power["lease"]!["adaptive"]);
        Assert.Equal(4, (int?)power["lease"]!["window"]);
        Assert.Equal(30000, (int?)power["lease"]!["maxMs"]);
        Assert.Equal(0, (int?)power["lease"]!["idleRevokeMs"]);
        Assert.False(power["lease"]!.AsObject().ContainsKey("minMs"));
        using (var h = AppMcpHub.Start(power.ToJsonString(), null))
        {
            Assert.Equal("fixed", h.Status().Lease!.Mode);
        }
        var badLease = new HubOptions { DisableIpc = true, DisableListen = true, Dispatcher = null, Lease = new LeaseOptions { Window = 0 } };
        Assert.Equal(HubStatus.InvalidConfig, Assert.Throws<HubException>(() => AppMcpHub.Start(badLease)).Status);
        using (var h = AppMcpHub.Start(exposure.ToJsonString(), null))
        {
            // 渐进暴露：没有展开的 App 时只有内置工具（含 apps.tools）
            var names = h.ListTools(new ToolFilter { Session = "c1" }).Select(t => t.Name).ToArray();
            Assert.Equal(["apps.list", "apps.select", "apps.overview", "apps.tools", "apps.activate", "apps.release", "apps.lock", "apps.unlock"], names);
        }

        var disabled = JsonNode.Parse(new HubOptions { DisableListen = true, DisableIpc = true }.ToConfigJson())!.AsObject();
        Assert.True(disabled.ContainsKey("listen"));
        Assert.Null(disabled["listen"]);
        Assert.True(disabled.ContainsKey("ipcEndpoint"));
        Assert.Null(disabled["ipcEndpoint"]);
        Assert.False(JsonNode.Parse(new HubOptions().ToConfigJson())!.AsObject().ContainsKey("ipcEndpoint"));
    }

    [Fact]
    public void PolicyConfigSerializesAndValidates()
    {
        var policy = new HubPolicy
        {
            Rules =
            {
                new HubPolicyRule { Id = "hide-x", Action = HubPolicyAction.Hide, App = "shop*" },
                new HubPolicyRule
                {
                    Id = "no-destructive",
                    Action = HubPolicyAction.Deny,
                    App = "*",
                    Annotations = new HubAnnotationMatch { DestructiveHint = true },
                    Agent = "cursor",
                    Hooks = [HubPolicyHook.Call, HubPolicyHook.Wake],
                },
            },
        };
        var json = JsonNode.Parse(new HubOptions { Policy = policy }.ToConfigJson())!["policy"]!["rules"]!.AsArray();
        Assert.Equal("""{"id":"hide-x","action":"hide","app":"shop*"}""", json[0]!.ToJsonString());
        Assert.Equal("""{"id":"no-destructive","action":"deny","app":"*","annotations":{"destructiveHint":true},"agent":"cursor","hooks":["call","wake"]}""",
            json[1]!.ToJsonString());
        Assert.False(JsonNode.Parse(new HubOptions().ToConfigJson())!.AsObject().ContainsKey("policy"));

        using (var h = AppMcpHub.Start(new HubOptions { DisableIpc = true, DisableListen = true, Dispatcher = null, Policy = policy }))
        {
            var st = h.Status().Policy!;
            Assert.Equal(["hide-x", "no-destructive"], st.Rules.Select(r => r.Id));
            Assert.Equal(HubPolicyAction.Deny, st.Rules[1].Action);
            Assert.Equal([HubPolicyHook.Call, HubPolicyHook.Wake], st.Rules[1].Hooks!);
            Assert.True(st.Rules[1].Annotations!.DestructiveHint);
            Assert.Equal("cursor", st.Rules[1].Agent);
            Assert.Equal(0UL, st.Rules[0].Hits);
            Assert.Null(st.LastError);
        }
        // 通配符不在末尾 → 启动失败
        var bad = new HubOptions
        {
            DisableIpc = true,
            DisableListen = true,
            Dispatcher = null,
            Policy = new HubPolicy { Rules = { new HubPolicyRule { Id = "x", Action = HubPolicyAction.Hide, App = "a*b" } } },
        };
        Assert.Equal(HubStatus.InvalidConfig, Assert.Throws<HubException>(() => AppMcpHub.Start(bad)).Status);
    }

    [Fact]
    public void StatelessConfigSerializesAndStarts()
    {
        var options = new HubOptions
        {
            DisableIpc = true,
            DisableListen = true,
            Dispatcher = null,
            TaskIdleTtl = TimeSpan.Zero,
            StatelessToolExposure = ToolExposure.Progressive,
            PrincipalSelectTtl = TimeSpan.FromMilliseconds(1500),
            StatelessListTtl = TimeSpan.FromMilliseconds(750),
        };
        var json = JsonNode.Parse(options.ToConfigJson())!.AsObject();
        Assert.Equal(0, (long?)json["taskIdleTtlMs"]);
        Assert.Equal("progressive", (string?)json["statelessToolExposure"]);
        Assert.Equal(1500, (long?)json["principalSelectTtlMs"]);
        Assert.Equal(750, (long?)json["statelessListTtlMs"]);
        var defaults = JsonNode.Parse(new HubOptions().ToConfigJson())!.AsObject();
        Assert.False(defaults.ContainsKey("taskIdleTtlMs") || defaults.ContainsKey("statelessToolExposure")
            || defaults.ContainsKey("principalSelectTtlMs") || defaults.ContainsKey("statelessListTtlMs"));
        Assert.Throws<ArgumentOutOfRangeException>(() => new HubOptions { TaskIdleTtl = TimeSpan.FromSeconds(-1) }.ToConfigJson());
        using var hub = AppMcpHub.Start(options);
        Assert.Empty(hub.Status().Tasks!);
    }

    [Fact]
    public void McpListenConfigSerializesAndStarts()
    {
        var options = new HubOptions
        {
            DisableIpc = true,
            DisableListen = true,
            Dispatcher = null,
            McpProtocolMode = McpProtocolMode.LegacyOnly,
            MaxListenStreams = 0,
            MaxListenResources = 8,
        };
        var json = JsonNode.Parse(options.ToConfigJson())!.AsObject();
        Assert.Equal("legacyOnly", (string?)json["mcpProtocolMode"]);
        Assert.Equal(0, (int?)json["maxListenStreams"]);
        Assert.Equal(8, (int?)json["maxListenResources"]);
        Assert.Equal("auto", (string?)JsonNode.Parse(new HubOptions { McpProtocolMode = McpProtocolMode.Auto }.ToConfigJson())!["mcpProtocolMode"]);
        var defaults = JsonNode.Parse(new HubOptions().ToConfigJson())!.AsObject();
        Assert.False(defaults.ContainsKey("mcpProtocolMode") || defaults.ContainsKey("maxListenStreams")
            || defaults.ContainsKey("maxListenResources"));
        Assert.Throws<ArgumentOutOfRangeException>(() => new HubOptions { MaxListenStreams = -1 }.ToConfigJson());
        Assert.Throws<ArgumentOutOfRangeException>(() => new HubOptions { MaxListenResources = -1 }.ToConfigJson());
        Assert.Throws<ArgumentOutOfRangeException>(() => new HubOptions { McpProtocolMode = (McpProtocolMode)9 }.ToConfigJson());
        using var hub = AppMcpHub.Start(options);
        Assert.Equal(0, hub.Status().McpListenStreams);
        var old = JsonSerializer.Deserialize<HubStatusInfo>("""
            {"service":"app-mcp","version":"0","pid":1,"startedAtMs":0,"mcpHttp":true,
             "auth":{"tokenConfigured":false,"tokenRequiredWithoutOrigin":false},"mcpSessions":1,"apps":[],"reports":[]}
            """, AppMcpHub.WireOptions)!;
        Assert.Null(old.McpListenStreams);
    }

    [Fact]
    public void MaxTaskHandlesSerializesAndStarts()
    {
        var json = JsonNode.Parse(new HubOptions { MaxTaskHandles = 0 }.ToConfigJson())!.AsObject();
        Assert.Equal(0, (int?)json["maxTaskHandles"]);
        Assert.Equal(5, (int?)JsonNode.Parse(new HubOptions { MaxTaskHandles = 5 }.ToConfigJson())!["maxTaskHandles"]);
        Assert.False(JsonNode.Parse(new HubOptions().ToConfigJson())!.AsObject().ContainsKey("maxTaskHandles"));
        Assert.Throws<ArgumentOutOfRangeException>(() => new HubOptions { MaxTaskHandles = -1 }.ToConfigJson());
        using var hub = AppMcpHub.Start(new HubOptions { DisableIpc = true, DisableListen = true, Dispatcher = null, MaxTaskHandles = 0 });
        Assert.Equal(0, hub.Status().McpSessions);
    }

    [Fact]
    public void StatusParsesAgentTasksAndApprovalPrincipal()
    {
        var status = JsonSerializer.Deserialize<HubStatusInfo>("""
            {"service":"app-mcp","version":"0","pid":1,"startedAtMs":0,"mcpHttp":true,
             "auth":{"tokenConfigured":false,"tokenRequiredWithoutOrigin":false},"mcpSessions":1,"apps":[],"reports":[],
             "tasks":[{"id":"task-1","caller":"principal:local","kind":"principal",
                       "selections":[{"appId":"shop","instanceId":"a","expiresInMs":900}],
                       "leases":[{"connectionId":"c-1","expiresInMs":500}],"inflight":2,"idleMs":3}]}
            """, AppMcpHub.WireOptions)!;
        var task = Assert.Single(status.Tasks!);
        Assert.Equal(("task-1", "principal:local", "principal", 2U, (ulong?)3), (task.Id, task.Caller, task.Kind, task.Inflight, task.IdleMs));
        Assert.Equal(new TaskSelectionStatusInfo("shop", "a") { ExpiresInMs = 900 }, Assert.Single(task.Selections));
        Assert.Equal(new TaskLeaseStatusInfo("c-1", 500), Assert.Single(task.Leases));
        Assert.Null(task.Agent);
        Assert.Null(status.Usage);
        // 第 16 项 N5 / P3：agents、tasks[].agent 与 usage
        var withUsage = JsonSerializer.Deserialize<HubStatusInfo>("""
            {"service":"app-mcp","version":"0","pid":1,"startedAtMs":0,"mcpHttp":true,
             "auth":{"tokenConfigured":false,"tokenRequiredWithoutOrigin":false},"mcpSessions":0,"apps":[],"reports":[],
             "agents":["claude"],
             "tasks":[{"id":"task-2","caller":"principal:agent:claude","kind":"principal","agent":"claude","selections":[],"leases":[],"inflight":0}],
             "usage":[{"subject":"agent:claude","agent":"claude","calls":3,"wakes":1,"rateLimited":2,"argumentsBytes":10,"resultBytes":20,
                       "apps":[{"appId":"shop","calls":3,"wakes":1,"rateLimited":2,"argumentsBytes":10,"resultBytes":20}]}]}
            """, AppMcpHub.WireOptions)!;
        Assert.Equal(["claude"], withUsage.Agents!);
        Assert.Equal("claude", Assert.Single(withUsage.Tasks!).Agent);
        var usage = Assert.Single(withUsage.Usage!);
        Assert.Equal(("agent:claude", "claude", 3UL, 1UL, 2UL, false), (usage.Subject, usage.Agent, usage.Calls, usage.Wakes, usage.RateLimited, usage.AppsTruncated));
        Assert.Equal(new AppUsageStatusInfo("shop", 3, 1, 2, 10, 20), Assert.Single(usage.Apps));
        var approval = JsonSerializer.Deserialize<ApprovalRequest>("""
            {"callId":"c","appId":"a","appName":"A","tool":"t","title":null,"description":"d","risk":"write","arguments":{},
             "session":"principal:local","principal":"local","clientName":"claude-code"}
            """, AppMcpHub.WireOptions)!;
        Assert.Equal(("local", "claude-code"), (approval.Principal, approval.ClientName));
    }

    [Fact]
    public void LimitsAndOutputValidationConfig()
    {
        var json = JsonNode.Parse(new HubOptions
        {
            Limits = new HubLimits { ToolRatePerMinute = 10, ToolRateBurst = 2, MaxResultBytes = 0 },
            OutputValidation = OutputValidation.Reject,
        }.ToConfigJson())!.AsObject();
        Assert.Equal(10, (int?)json["limits"]!["toolRatePerMinute"]);
        Assert.Equal(2, (int?)json["limits"]!["toolRateBurst"]);
        Assert.Equal(0, (int?)json["limits"]!["maxResultBytes"]);
        Assert.False(json["limits"]!.AsObject().ContainsKey("appRatePerMinute"));
        Assert.Equal("reject", (string?)json["outputValidation"]);
        Assert.False(JsonNode.Parse(new HubOptions().ToConfigJson())!.AsObject().ContainsKey("limits"));

        using (var h = AppMcpHub.Start(new HubOptions
        {
            DisableIpc = true,
            DisableListen = true,
            Dispatcher = null,
            Limits = new HubLimits { ToolRatePerMinute = 10, ToolRateBurst = 2 },
            OutputValidation = OutputValidation.Off,
        }))
        {
            var status = h.Status();
            Assert.Equal(10U, status.Limits!.ToolRatePerMinute);
            Assert.Equal(2U, status.Limits.ToolRateBurst);
            Assert.Equal(600U, status.Limits.AppRatePerMinute); // 缺省字段取默认值
            Assert.Equal(4UL * 1024 * 1024, status.Limits.MaxResultBytes);
            Assert.Equal(OutputValidation.Off, status.OutputValidation);
        }
        using (var h = AppMcpHub.Start(new HubOptions { DisableIpc = true, DisableListen = true, Dispatcher = null }))
        {
            Assert.Equal(OutputValidation.Log, h.Status().OutputValidation);
        }
        // 限流时 burst = 0 被拒；不限（perMinute = 0）时允许
        var bad = new HubOptions { DisableIpc = true, DisableListen = true, Dispatcher = null, Limits = new HubLimits { ToolRateBurst = 0 } };
        Assert.Equal(HubStatus.InvalidConfig, Assert.Throws<HubException>(() => AppMcpHub.Start(bad)).Status);
        using (AppMcpHub.Start(new HubOptions
        {
            DisableIpc = true,
            DisableListen = true,
            Dispatcher = null,
            Limits = new HubLimits { AppRatePerMinute = 0, AppRateBurst = 0 },
        }))
        {
        }
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
    public void StateDirReportsDormantStore()
    {
        var dir = Path.Combine(Path.GetTempPath(), $"app-mcp-cs-state-{Environment.ProcessId}");
        if (Directory.Exists(dir)) Directory.Delete(dir, true);
        Directory.CreateDirectory(Path.Combine(dir, "dormant"));
        File.WriteAllText(Path.Combine(dir, "dormant", "broken.json"), "{");
        try
        {
            using var hub = AppMcpHub.Start(new HubOptions { DisableListen = true, DisableIpc = true, Dispatcher = null, StateDir = dir });
            var store = hub.Status().DormantStore;
            Assert.NotNull(store);
            Assert.Equal(Path.Combine(dir, "dormant"), store!.Dir);
            Assert.Equal(0UL, store.LoadedInstances);
            Assert.Equal(0UL, store.Writes);
            Assert.Equal("broken.json", Assert.Single(store.Issues).File);
            Assert.Null(store.LastError);
        }
        finally
        {
            Directory.Delete(dir, true);
        }
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
        Assert.NotNull(status.Lease);
        Assert.Equal("adaptive", status.Lease!.Mode);
        Assert.Equal(60000UL, status.Lease.DefaultMs);
        Assert.Equal(20U, status.Lease.Window);
        Assert.Empty(status.Lease.Pairs);
        Assert.Null(status.DormantStore);
        Assert.NotNull(status.Tasks);
        Assert.Empty(status.Tasks!);
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
        // Hub API 会话的 Agent 任务（spec/hub-api.md 3.6）
        var task = Assert.Single(hub.Status().Tasks!, t => t.Caller == "api:s1");
        Assert.Equal("api", task.Kind);
        Assert.StartsWith("task-", task.Id);

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
        // Principal / ClientName 只在 MCP 出口发起的审批中出现
        Assert.Null(req.Principal);
        Assert.Null(req.ClientName);
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
    public async Task AnnotationsStructuredResultAndRateLimit()
    {
        await using var hub = AppMcpHub.Start(new HubOptions
        {
            DisableIpc = true,
            Listen = "127.0.0.1:0",
            Dispatcher = null,
            RequireApprovalAtOrAbove = HubRisk.Write,
            Limits = new HubLimits { ToolRatePerMinute = 1, ToolRateBurst = 1 },
        });
        var approvals = new ConcurrentQueue<ApprovalRequest>();
        hub.ApprovalHandler = (req, _) =>
        {
            approvals.Enqueue(req);
            return Task.FromResult(true);
        };
        await using var app = AppMcp.AppMcpClient.Create(new AppMcp.AppMcpClientOptions
        {
            AppId = "shop",
            AppName = "商店",
            HostUrl = $"ws://{hub.ListenAddress}/app",
            Dispatcher = null,
        });
        using var submit = app.RegisterTool("order.submit", "下单", (_, _) =>
            Task.FromResult<object?>(new AppMcp.ToolResult(new { orderId = "o1" })
            {
                Status = AppMcp.ToolResultStatus.Pending,
                StateResource = "order.state",
                Summary = "已提交",
                Annotations = new AppMcp.ContentAnnotations { Audience = [AppMcp.ContentAudience.User], Priority = 0.5 },
            }), new AppMcp.ToolOptions
            {
                Annotations = new AppMcp.ToolAnnotations { IdempotentHint = false },
                OutputSchemaJson = """{"type":"object","properties":{"orderId":{"type":"string"}}}""",
            });
        app.Start();
        await WaitUntil(() => hub.ListTools(new ToolFilter { Apps = ["shop"], OnlyAvailable = true, IncludeBuiltin = false }).Count == 1,
            "工具未同步");

        var tool = Assert.Single(hub.ListTools(new ToolFilter { Apps = ["shop"], IncludeBuiltin = false }));
        // 声明的字段优先，缺少的按 risk（write）推导
        Assert.False(tool.Annotations!.IdempotentHint);
        Assert.False(tool.Annotations.ReadOnlyHint);
        Assert.Equal("string", tool.OutputSchema!.Value.GetProperty("properties").GetProperty("orderId").GetProperty("type").GetString());

        var outcome = await hub.CallAsync("shop.order.submit");
        Assert.True(outcome.IsSuccess, outcome.Json.GetRawText());
        Assert.Equal("o1", outcome.Data!.Value.GetProperty("orderId").GetString());
        Assert.Equal(HubResultStatus.Pending, outcome.Status);
        Assert.Equal("app-mcp://shop/order.state", outcome.StateResource);
        Assert.Equal("已提交", outcome.Summary);
        Assert.Equal(["user"], outcome.Annotations!.Audience!);
        Assert.Equal(0.5, outcome.Annotations.Priority);
        var approval = Assert.Single(approvals);
        Assert.False(approval.Annotations!.IdempotentHint);

        // 突发 1：第二次立即调用被限流
        var limited = await hub.CallAsync("shop.order.submit");
        Assert.Equal(HubError.RateLimited, limited.Error?.Kind);
        Assert.Equal("tool", limited.Error!.Details!.Value.GetProperty("scope").GetString());

        var shop = Assert.Single(hub.Status().Apps, a => a.AppId == "shop");
        Assert.Equal(1UL, shop.RateLimited);
        Assert.Equal(0UL, shop.TooLarge);
        var decl = Assert.Single(shop.Tools);
        Assert.Equal("order.submit", decl.Name);
        Assert.Equal("write", decl.Risk);
        Assert.True(decl.OutputSchema);
        Assert.Equal(new HubToolAnnotations(null, null, null, false, null), decl.Annotations);
        Assert.False(decl.Effective.ReadOnlyHint);
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
        Assert.True(r.Woke, r.Json.GetRawText()); // 休眠实例经唤醒回连后才送达（spec/hub-api.md 3.15）
        Assert.True(r.DurationMs >= 0);
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
            // 功耗观测（4e）
            Assert.NotNull(inst.Power);
            Assert.Equal(0UL, inst.Power!.Reconnects);
            Assert.Equal(0UL, Assert.Single(status.Apps, a => a.AppId == "notes").Wakes);
        }
        finally
        {
            if (Directory.Exists(dir)) Directory.Delete(dir, true);
        }
    }

    [Fact]
    public async Task PolicyHideDenyAndSetPolicy()
    {
        await using var hub = AppMcpHub.Start(new HubOptions
        {
            DisableIpc = true,
            Listen = "127.0.0.1:0",
            Dispatcher = null,
            Policy = new HubPolicy
            {
                Rules =
                {
                    new HubPolicyRule { Id = "hide-clear", Action = HubPolicyAction.Hide, App = "shop", Tool = "cart.clear" },
                    new HubPolicyRule { Id = "no-pay", Action = HubPolicyAction.Deny, App = "shop", Tool = "order.pay" },
                },
            },
        });
        await using var app = AppMcp.AppMcpClient.Create(new AppMcp.AppMcpClientOptions
        {
            AppId = "shop",
            AppName = "商店",
            HostUrl = $"ws://{hub.ListenAddress}/app",
            Dispatcher = null,
        });
        Task<object?> Ok(object? data) => Task.FromResult<object?>(data);
        using var add = app.RegisterTool("cart.add", "加购", (_, _) => Ok(new { added = true }));
        using var clear = app.RegisterTool("cart.clear", "清空", (_, _) => Ok(new { cleared = true }));
        using var pay = app.RegisterTool("order.pay", "付款", (_, _) => Ok(new { paid = true }));
        app.Start();
        var filter = new ToolFilter { Apps = ["shop"], IncludeBuiltin = false };
        List<string> Names() => hub.ListTools(filter).Select(t => t.Name).Order().ToList();
        await WaitUntil(() => Names().Count == 2, "工具未同步");
        Assert.Equal(["shop.cart.add", "shop.order.pay"], Names());

        // (a) hide：不在列表，调用为 TOOL_NOT_FOUND
        var hidden = await hub.CallAsync("shop.cart.clear");
        Assert.Equal("TOOL_NOT_FOUND", hidden.Error?.Kind);
        // (b) deny：POLICY_DENIED，details.ruleId
        var denied = await hub.CallAsync("shop.order.pay");
        Assert.Equal(HubError.PolicyDenied, denied.Error?.Kind);
        Assert.Equal("no-pay", denied.Error!.Details!.Value.GetProperty("ruleId").GetString());
        Assert.Equal("call", denied.Error.Details.Value.GetProperty("hook").GetString());
        var st = hub.Status().Policy!;
        Assert.Equal([1UL, 1UL], st.Rules.Select(r => r.Hits));

        // (c) 不合法（hide 不能写 hooks）→ 抛出，之前的规则继续生效
        var invalid = new HubPolicy
        {
            Rules = { new HubPolicyRule { Id = "x", Action = HubPolicyAction.Hide, App = "shop", Hooks = [HubPolicyHook.Call] } },
        };
        var e = Assert.Throws<HubException>(() => hub.SetPolicy(invalid));
        Assert.Equal(HubStatus.InvalidConfig, e.Status);
        Assert.Contains("hooks", e.Message);
        Assert.Equal(HubError.PolicyDenied, (await hub.CallAsync("shop.order.pay")).Error?.Kind);
        Assert.NotNull(hub.Status().Policy!.LastError);

        // 清空：恢复原行为
        hub.SetPolicy(new HubPolicy());
        Assert.Equal(["shop.cart.add", "shop.cart.clear", "shop.order.pay"], Names());
        var paid = await hub.CallAsync("shop.order.pay");
        Assert.True(paid.IsSuccess, paid.Json.GetRawText());
        Assert.Empty(hub.Status().Policy!.Rules);
        Assert.Null(hub.Status().Policy!.LastError);
    }

    /// <summary>同步收集进度（不经同步上下文转发）。</summary>
    /// <summary>spec/hub-api.md 3.14 / 3.15：HubToolInfo.Surface / Page、CallRequest.IdempotencyKey 原样转交、CallOutcome.RoutedTo。</summary>
    [Fact]
    public async Task SurfacePageAndIdempotencyKey()
    {
        await using var hub = AppMcpHub.Start(new HubOptions
        {
            Listen = "127.0.0.1:0",
            DisableIpc = true,
            NavigateTimeout = TimeSpan.FromMilliseconds(800),
            Dispatcher = null,
        });
        await using var app = AppMcp.AppMcpClient.Create(new AppMcp.AppMcpClientOptions
        {
            AppId = "cafe",
            AppName = "咖啡",
            HostUrl = $"ws://{hub.ListenAddress}/app",
            Dispatcher = null,
        });
        Func<JsonElement, AppMcp.ToolContext, Task<object?>> EchoKey = (_, ctx) => Task.FromResult<object?>(new { key = ctx.IdempotencyKey });
        using var checkout = app.RegisterTool("cart.checkout", "结算", EchoKey,
            new AppMcp.ToolOptions { Surface = AppMcp.ToolSurface.View, Page = "cart" });
        using var submit = app.RegisterTool("order.submit", "下单", EchoKey);
        app.Start();
        await WaitUntil(() => hub.ListTools(new ToolFilter { Apps = ["cafe"], OnlyAvailable = true, IncludeBuiltin = false }).Count == 2,
            "工具未同步");
        var tools = hub.ListTools(new ToolFilter { Apps = ["cafe"], IncludeBuiltin = false });
        var view = tools.Single(t => t.Tool == "cart.checkout");
        Assert.Equal((HubToolSurface.View, "cart"), (view.Surface, view.Page));
        var plain = tools.Single(t => t.Tool == "order.submit");
        Assert.Equal((HubToolSurface.App, (string?)null), (plain.Surface, plain.Page));
        var builtins = hub.ListTools().Where(t => t.Name.StartsWith("apps.", StringComparison.Ordinal)).ToList();
        foreach (var n in new[] { "apps.activate", "apps.release", "apps.page", "apps.navigate" })
        {
            Assert.Contains(builtins, t => t.Name == n);
        }
        Assert.All(builtins, t => Assert.True(t.Surface is null && t.Page is null, t.Name));

        var out1 = await hub.CallAsync(new CallRequest("cafe.order.submit") { IdempotencyKey = "order-7" });
        Assert.True(out1.IsSuccess, out1.Json.ToString());
        Assert.Equal("order-7", out1.Data!.Value.GetProperty("key").GetString());
        Assert.Null(out1.RoutedTo);
        Assert.False(out1.Woke, out1.Json.GetRawText()); // 已连接：不经唤醒
        Assert.True(out1.Json.TryGetProperty("durationMs", out _), out1.Json.GetRawText());
        var bad = await hub.CallAsync(new CallRequest("cafe.order.submit") { IdempotencyKey = "" });
        Assert.Equal("INVALID_INPUT", bad.Error?.Kind);
        app.Stop();
    }

    private sealed class ProgressLog : IProgress<CallProgress>
    {
        public ConcurrentQueue<CallProgress> Items { get; } = new();
        public void Report(CallProgress value) => Items.Enqueue(value);
    }

    [Fact]
    public async Task CallWithProgressReportsBeforeResult()
    {
        await using var hub = AppMcpHub.Start(new HubOptions
        {
            DisableIpc = true,
            Listen = "127.0.0.1:0",
            Dispatcher = null,
            ProgressInterval = TimeSpan.FromMilliseconds(10),
        });
        await using var app = AppMcp.AppMcpClient.Create(new AppMcp.AppMcpClientOptions
        {
            AppId = "steps",
            AppName = "步骤",
            HostUrl = $"ws://{hub.ListenAddress}/app",
            Dispatcher = null,
        });
        using var run = app.RegisterTool("run", "分步执行", async (_, ctx) =>
        {
            ctx.Progress(1, 2, "第一步");
            await Task.Delay(150);
            ctx.Progress(2);
            await Task.Delay(150);
            return (object?)new { done = true };
        });
        app.Start();
        await WaitUntil(() => hub.ListTools(new ToolFilter { Apps = ["steps"], OnlyAvailable = true, IncludeBuiltin = false }).Count == 1,
            "工具未同步");

        var log = new ProgressLog();
        var outcome = await hub.CallAsync(new CallRequest("steps.run") { CallId = "p1" }, log);
        Assert.True(outcome.IsSuccess, outcome.Json.GetRawText());
        Assert.True(outcome.Data!.Value.GetProperty("done").GetBoolean());
        Assert.Equal(
            [new CallProgress("p1", 1, 2, "第一步"), new CallProgress("p1", 2, null, null)],
            log.Items.ToArray());

        // progress 为 null：等同普通调用
        var plain = await hub.CallAsync(new CallRequest("steps.run"), null);
        Assert.True(plain.IsSuccess, plain.Json.GetRawText());
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
