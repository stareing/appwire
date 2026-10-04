using System.Diagnostics;
using System.Text.Json;
using System.Text.Json.Serialization;
using Xunit.Abstractions;

namespace AppMcp.Tests;

/// <summary>
/// 一致性用例 runner（.NET）：按 <c>conformance/cases/*.json</c> 的 app 部分注册工具与资源，连接 fake_host
/// （<c>--case</c> 模式，核对在 fake_host 内完成），汇总各用例结论。格式与约定见 conformance/README.md，
/// 结构对照 crates/native/tests/it/conformance.rs。
/// 只跑部分用例：<c>APP_MCP_CONFORMANCE_CASES=handshake,errors dotnet test --filter ConformanceTests</c>。
/// </summary>
public class ConformanceTests(ITestOutputHelper output)
{
    private const string Sdk = "dotnet";

    /// <summary>本 runner 支持的用例能力（<c>requires</c>），见 conformance/README.md 第 4 节。</summary>
    private static readonly HashSet<string> Features =
    [
        "toolOptions", "mutate", "lifecycle", "wake", "richResult", "userAction", "progress", "resourceOptions", "readFailure",
        "surface", "navigation", "backgroundTool", "backgroundNavigation", "idempotencyKey",
        "callScheduling", "busy", "events", "implements", "cache", "deprecated",
    ];

    private static readonly IReadOnlyDictionary<string, ToolRisk> Risks = new Dictionary<string, ToolRisk>
    {
        ["read"] = ToolRisk.Read, ["write"] = ToolRisk.Write, ["destructive"] = ToolRisk.Destructive,
        ["payment"] = ToolRisk.Payment, ["os-sensitive"] = ToolRisk.OsSensitive,
    };

    private static readonly IReadOnlyDictionary<string, ToolActivation> Activations = new Dictionary<string, ToolActivation>
    {
        ["headless"] = ToolActivation.Headless, ["background"] = ToolActivation.Background, ["foreground"] = ToolActivation.Foreground,
    };

    private static readonly IReadOnlyDictionary<string, ToolResultStatus> Statuses = new Dictionary<string, ToolResultStatus>
    {
        ["done"] = ToolResultStatus.Done, ["pending"] = ToolResultStatus.Pending,
        ["partial"] = ToolResultStatus.Partial, ["noop"] = ToolResultStatus.Noop,
    };

    private static readonly IReadOnlyDictionary<string, LifecycleMode> Modes = new Dictionary<string, LifecycleMode>
    {
        ["persistent"] = LifecycleMode.Persistent, ["idle"] = LifecycleMode.Idle, ["on-demand"] = LifecycleMode.OnDemand,
    };

    private static readonly IReadOnlyDictionary<string, AppVisibility> Visibilities = new Dictionary<string, AppVisibility>
    {
        ["visible"] = AppVisibility.Visible, ["hidden"] = AppVisibility.Hidden, ["frozen"] = AppVisibility.Frozen,
    };

    /// <summary>协议注解（camelCase 字段与枚举值）→ SDK 类型。</summary>
    private static readonly JsonSerializerOptions ProtocolJson = new()
    {
        Converters = { new JsonStringEnumConverter(JsonNamingPolicy.CamelCase) },
    };

    [Fact]
    public void ConformanceCases()
    {
        var fakeHost = FakeHost.Locate(output);
        if (fakeHost is null) return;
        var root = FindRepoRoot() ?? throw new InvalidOperationException("找不到仓库根目录");
        var reportDir = Path.Combine(root, "target", "conformance");
        var only = Environment.GetEnvironmentVariable("APP_MCP_CONFORMANCE_CASES")?.Split(',', StringSplitOptions.RemoveEmptyEntries);
        var cases = Directory.GetFiles(Path.Combine(root, "conformance", "cases"), "*.json")
            .Where(p => only is null || only.Contains(Path.GetFileNameWithoutExtension(p)))
            .Order(StringComparer.Ordinal)
            .ToList();
        Assert.NotEmpty(cases);

        var failed = new List<string>();
        foreach (var path in cases)
        {
            var (status, failures) = RunCase(fakeHost, path, reportDir);
            output.WriteLine($"[{Sdk}] {Path.GetFileNameWithoutExtension(path),-24} {status}");
            if (status is not ("pass" or "xfail" or "xpass" or "skip")) failed.Add($"{path}: {failures}");
        }
        Assert.True(failed.Count == 0, "一致性用例失败：\n" + string.Join("\n", failed));
    }

    /// <summary>跑一个用例，返回 fake_host 给出的结论。</summary>
    private (string Status, string Failures) RunCase(string fakeHost, string path, string reportDir)
    {
        using var doc = JsonDocument.Parse(File.ReadAllText(path));
        var kase = doc.RootElement.Clone();
        var missing = Items(Get(kase, "requires")).Select(r => r.GetString()!).Where(f => !Features.Contains(f)).ToList();

        var psi = new ProcessStartInfo(fakeHost)
        {
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            UseShellExecute = false,
            StandardOutputEncoding = System.Text.Encoding.UTF8,
            StandardErrorEncoding = System.Text.Encoding.UTF8,
        };
        foreach (var a in new[] { "--case", path, "--sdk", Sdk, "--report-dir", reportDir }) psi.ArgumentList.Add(a);
        if (missing.Count > 0)
        {
            psi.ArgumentList.Add("--skip");
            psi.ArgumentList.Add("runner 不支持：" + string.Join(", ", missing));
        }
        using var process = Process.Start(psi)!;
        var stderr = new List<string>();
        process.ErrorDataReceived += (_, e) =>
        {
            if (e.Data is not null) lock (stderr) stderr.Add(e.Data);
        };
        process.BeginErrorReadLine();

        CaseApp? app = null;
        (string, string) verdict = ("error", "fake_host 没有输出结论");
        try
        {
            while (process.StandardOutput.ReadLine() is { } line)
            {
                if (line.StartsWith("LISTENING ", StringComparison.Ordinal))
                {
                    app = new CaseApp(Config(line["LISTENING ".Length..].Trim(), Get(Get(kase, "app"), "config")));
                    foreach (var t in Items(Get(Get(kase, "app"), "tools"))) app.RegisterTool(t);
                    foreach (var r in Items(Get(Get(kase, "app"), "resources"))) app.RegisterResource(r);
                    foreach (var e in Items(Get(Get(kase, "app"), "events"))) app.DeclareEvent(e);
                    if (Get(Get(kase, "app"), "navigation").ValueKind == JsonValueKind.Object) app.SetNavigation(Get(Get(kase, "app"), "navigation"));
                    if (Text(Get(Get(kase, "app"), "visibility")) is { } visibility) app.Client.SetVisibility(Visibilities[visibility], false);
                    if (Get(Get(kase, "app"), "busy").ValueKind is JsonValueKind.True or JsonValueKind.False) app.Client.SetBusy(Get(Get(kase, "app"), "busy").GetBoolean());
                    app.Client.Start();
                    continue;
                }
                if (!line.StartsWith('{')) continue;
                using var msg = JsonDocument.Parse(line);
                var type = Get(msg.RootElement, "type").ValueKind == JsonValueKind.String ? msg.RootElement.GetProperty("type").GetString() : null;
                if (type == "wake") app?.Client.HandleWake(msg.RootElement.GetProperty("arg").GetString() ?? "");
                if (type == "verdict")
                {
                    verdict = (msg.RootElement.GetProperty("status").GetString() ?? "error", Get(msg.RootElement, "failures").ToString());
                }
            }
            process.WaitForExit();
        }
        finally
        {
            if (!process.HasExited) process.Kill();
            app?.Dispose();
        }
        lock (stderr)
        {
            foreach (var l in stderr) output.WriteLine("  " + l);
        }
        if (process.ExitCode != 0 && verdict.Item1 != "fail") return ("error", $"fake_host 退出码 {process.ExitCode}");
        return verdict;
    }

    private static AppMcpClientOptions Config(string addr, JsonElement c)
    {
        var tcp = addr.Contains(':') && !addr.StartsWith("unix:", StringComparison.Ordinal) && !addr.StartsWith("pipe:", StringComparison.Ordinal);
        var l = Get(c, "lifecycle");
        var lifecycle = new LifecycleOptions();
        if (Text(Get(l, "mode")) is { } mode) lifecycle = lifecycle with { Mode = Modes[mode] };
        if (Millis(Get(l, "idleTimeoutMs")) is { } idle) lifecycle = lifecycle with { IdleTimeout = idle };
        if (Millis(Get(l, "graceMs")) is { } grace) lifecycle = lifecycle with { Grace = grace };
        if (Millis(Get(l, "mergeWindowMs")) is { } merge) lifecycle = lifecycle with { MergeWindow = merge };
        var d = Get(c, "callDedup");
        var dedup = new CallDedupOptions();
        if (Millis(Get(d, "ttlMs")) is { } ttl) dedup = dedup with { Ttl = ttl };
        if (Get(d, "maxEntries").ValueKind == JsonValueKind.Number) dedup = dedup with { MaxEntries = Get(d, "maxEntries").GetInt32() };
        return new AppMcpClientOptions
        {
            AppId = "conf",
            AppName = "Conformance",
            HostUrl = tcp ? $"ws://{addr}/app" : addr,
            Dispatcher = null,
            Lifecycle = lifecycle,
            CallDedup = dedup,
            MaxConcurrentCalls = Get(c, "maxConcurrentCalls").ValueKind == JsonValueKind.Number ? Get(c, "maxConcurrentCalls").GetInt32() : 1,
            MaxQueuedCalls = Get(c, "maxQueuedCalls").ValueKind == JsonValueKind.Number ? Get(c, "maxQueuedCalls").GetInt32() : 64,
            BusyPolicy = Text(Get(c, "busyPolicy")) switch
            {
                null or "reject" => BusyPolicy.Reject,
                "queue" => BusyPolicy.Queue,
                var other => throw new InvalidOperationException("未知的 busyPolicy " + other),
            },
            NavigateInBackground = Get(c, "navigateInBackground").ValueKind switch
            {
                JsonValueKind.True => true,
                JsonValueKind.False => false,
                _ => null,
            },
        };
    }

    // ---- 用例 JSON 的读取 -------------------------------------------------------

    /// <summary>对象成员；不存在（或不是对象）时为 Undefined。</summary>
    private static JsonElement Get(JsonElement v, string key) =>
        v.ValueKind == JsonValueKind.Object && v.TryGetProperty(key, out var x) ? x : default;

    private static bool Has(JsonElement v, string key) => Get(v, key).ValueKind != JsonValueKind.Undefined;

    private static bool IsSet(JsonElement v) => v.ValueKind is not (JsonValueKind.Undefined or JsonValueKind.Null);

    private static IEnumerable<JsonElement> Items(JsonElement v) =>
        v.ValueKind == JsonValueKind.Array ? v.EnumerateArray() : Enumerable.Empty<JsonElement>();

    private static string? Text(JsonElement v) => v.ValueKind == JsonValueKind.String ? v.GetString() : null;

    private static TimeSpan? Millis(JsonElement v) =>
        v.ValueKind == JsonValueKind.Number ? TimeSpan.FromMilliseconds(v.GetDouble()) : null;

    private static string? Raw(JsonElement v) => IsSet(v) ? v.GetRawText() : null;

    private static T? As<T>(JsonElement v) where T : class => IsSet(v) ? v.Deserialize<T>(ProtocolJson) : null;

    private static ToolErrorKind ErrorKind(string? protocol) =>
        Enum.GetValues<ToolErrorKind>().FirstOrDefault(k => k.ToProtocolString() == protocol, ToolErrorKind.HandlerError);

    /// <summary>工具声明（用例 JSON）→ <see cref="ToolOptions"/>。</summary>
    private static ToolOptions Options(JsonElement decl) => new()
    {
        InputSchemaJson = Raw(Get(decl, "inputSchema")),
        Risk = Text(Get(decl, "risk")) is { } risk ? Risks[risk] : ToolRisk.Write,
        Activation = Text(Get(decl, "activation")) is { } a ? Activations[a] : null,
        Title = Text(Get(decl, "title")),
        Enabled = Get(decl, "enabled").ValueKind != JsonValueKind.False,
        Annotations = As<ToolAnnotations>(Get(decl, "annotations")),
        OutputSchemaJson = Raw(Get(decl, "outputSchema")),
        Surface = Text(Get(decl, "surface")) == "view" ? ToolSurface.View : ToolSurface.App,
        Page = Text(Get(decl, "page")),
        BackgroundTool = Text(Get(decl, "backgroundTool")),
        Concurrency = Get(decl, "concurrency").ValueKind == JsonValueKind.Number ? Get(decl, "concurrency").GetInt32() : 0,
        Exclusive = Text(Get(decl, "exclusive")),
        Implements = IsSet(Get(decl, "implements")) ? Items(Get(decl, "implements")).Select(i => i.GetString()!).ToList() : null,
        Cache = Cache(Get(decl, "cache")),
        Deprecated = Deprecation(Get(decl, "deprecated")),
    };

    /// <summary>用例 cache（<c>{ttlMs, scope?}</c>，spec/protocol.md 3.6）；未给出或 null 时为 null。</summary>
    /// <summary>用例 deprecated（<c>{message, replacement?, until?}</c>，spec/protocol.md 3.7）；未给出或 null 时为 null。</summary>
    private static ToolDeprecation? Deprecation(JsonElement v) => IsSet(v)
        ? new ToolDeprecation(Text(Get(v, "message")) ?? "", Text(Get(v, "replacement")), Text(Get(v, "until")))
        : null;

    private static CachePolicy? Cache(JsonElement v) => IsSet(v)
        ? new CachePolicy(Get(v, "ttlMs").GetUInt64(), Text(Get(v, "scope")) == "shared" ? CacheScope.Shared : CacheScope.Private)
        : null;

    private static string? FindRepoRoot()
    {
        var dir = new DirectoryInfo(AppContext.BaseDirectory);
        while (dir is not null)
        {
            if (Directory.Exists(Path.Combine(dir.FullName, "conformance", "cases"))) return dir.FullName;
            dir = dir.Parent;
        }
        return null;
    }

    /// <summary>一个用例的 App：客户端与已注册工具（mutate 用）。</summary>
    private sealed class CaseApp(AppMcpClientOptions options) : IDisposable
    {
        private readonly object _gate = new();
        private readonly Dictionary<string, (ToolRegistration Tool, JsonElement Decl)> _tools = new();
        private readonly List<ResourceRegistration> _resources = new();

        public AppMcpClient Client { get; } = AppMcpClient.Create(options);

        public void RegisterTool(JsonElement decl)
        {
            var spec = Get(decl, "handler");
            var runs = 0;
            var name = decl.GetProperty("name").GetString()!;
            var tool = Client.RegisterTool(name, decl.GetProperty("description").GetString()!,
                (args, ctx) => RunHandler(spec, Interlocked.Increment(ref runs), args, ctx), Options(decl));
            lock (_gate) _tools[name] = (tool, decl);
        }

        public void RegisterResource(JsonElement decl)
        {
            var spec = Get(decl, "read");
            _resources.Add(Client.RegisterResource(
                decl.GetProperty("name").GetString()!,
                decl.GetProperty("description").GetString()!,
                _ => ReadResource(spec),
                Text(Get(decl, "mimeType")),
                Get(decl, "realtime").ValueKind == JsonValueKind.True,
                As<ContentAnnotations>(Get(decl, "annotations")),
                Cache(Get(decl, "cache"))));
        }

        /// <summary>导航行为（conformance/README.md 2.4）。C# 最自然的写法：正常返回 = 完成，抛
        /// <see cref="NavigationDeniedException"/> = 拒绝，<see cref="UserActionRequiredException"/> = USER_ACTION_REQUIRED，其他异常 = 失败。</summary>
        public void SetNavigation(JsonElement pages) => Client.SetNavigationHandler(request =>
        {
            if (Get(pages, request.Page) is not { ValueKind: JsonValueKind.Object } spec)
                throw new InvalidOperationException("未知页面：" + request.Page);
            if (Text(Get(spec, "throw")) is { } crash) throw new InvalidOperationException(crash);
            foreach (var op in Items(Get(spec, "mutate"))) Mutate(op);
            if (Text(Get(spec, "deny")) is { } deny) throw new NavigationDeniedException(deny);
            if (Text(Get(spec, "fail")) is { } fail) throw new InvalidOperationException(fail);
            if (IsSet(Get(spec, "userAction")))
            {
                var u = Get(spec, "userAction");
                throw new UserActionRequiredException(Text(Get(u, "message")) ?? "", Text(Get(u, "reason")), Text(Get(u, "uri")));
            }
            if (Get(spec, "failParams").ValueKind == JsonValueKind.True) throw new InvalidOperationException(request.ParamsJson ?? "");
        });

        /// <summary>事件声明（conformance/README.md app.events 与变更 declareEvent）。</summary>
        public void DeclareEvent(JsonElement decl) => Client.DeclareEvent(
            Text(Get(decl, "name")) ?? "",
            Text(Get(decl, "description")) ?? "",
            Has(decl, "payloadSchema") ? Get(decl, "payloadSchema").GetRawText() : null);

        /// <summary>handler 的 <c>emit</c>：每项为 EmitEvent 的结果 true / false，本地错误（AppMcpException）为 "error"。</summary>
        private List<object> EmitEvents(JsonElement list) => Items(list).Select(e =>
        {
            try
            {
                return (object)Client.EmitEvent(Text(Get(e, "name")) ?? "", Has(e, "payload") ? Get(e, "payload") : null);
            }
            catch (AppMcpException)
            {
                return "error";
            }
        }).ToList();

        /// <summary>按 handler 描述执行（顺序：progress → delayMs → mutate → emit → 结果，见 conformance/README.md 2.1）。</summary>
        private async Task<object?> RunHandler(JsonElement spec, int count, JsonElement args, ToolContext ctx)
        {
            foreach (var p in Items(Get(spec, "progress")))
            {
                var total = Get(p, "total");
                ctx.Progress(Get(p, "progress").GetDouble(), total.ValueKind == JsonValueKind.Number ? total.GetDouble() : null, Text(Get(p, "message")));
            }
            if (Get(spec, "delayMs").ValueKind == JsonValueKind.Number)
            {
                try
                {
                    await Task.Delay(Get(spec, "delayMs").GetInt32(), ctx.CancellationToken);
                }
                catch (OperationCanceledException)
                {
                    // 被取消 / 超时后提前结束；之后的完成结果由 SDK 丢弃。
                }
            }
            foreach (var op in Items(Get(spec, "mutate"))) Mutate(op);
            var emitted = Get(spec, "emit").ValueKind == JsonValueKind.Array ? EmitEvents(Get(spec, "emit")) : null;
            return Complete(spec, count, args, ctx, emitted);
        }

        /// <summary>C# 最自然的写法：失败抛异常；无返回值即 handler 返回 null。</summary>
        private static object? Complete(JsonElement spec, int count, JsonElement args, ToolContext ctx, List<object>? emitted)
        {
            if (Text(Get(spec, "throw")) is { } message) throw new InvalidOperationException(message);
            if (IsSet(Get(spec, "userAction")))
            {
                var u = Get(spec, "userAction");
                throw new UserActionRequiredException(Text(Get(u, "message")) ?? "", Text(Get(u, "reason")), Text(Get(u, "uri")));
            }
            if (Get(spec, "result").ValueKind == JsonValueKind.Object)
            {
                var r = Get(spec, "result");
                return new ToolResult(IsSet(Get(r, "data")) ? Get(r, "data") : null)
                {
                    Status = Text(Get(r, "status")) is { } s ? Statuses[s] : ToolResultStatus.Done,
                    StateResource = Text(Get(r, "stateResource")),
                    Summary = Text(Get(r, "summary")),
                    Annotations = As<ContentAnnotations>(Get(r, "annotations")),
                    StateHints = Items(Get(r, "stateHints")).Select(h => h.GetString()!).ToList(),
                };
            }
            if (Has(spec, "return")) return Get(spec, "return");
            if (Get(spec, "echo").ValueKind == JsonValueKind.True) return args;
            if (Get(spec, "returnIdempotencyKey").ValueKind == JsonValueKind.True)
                return new Dictionary<string, string?> { ["idempotencyKey"] = ctx.IdempotencyKey };
            if (Get(spec, "counter").ValueKind == JsonValueKind.True) return new { count };
            if (emitted is not null) return new { emitted };
            // returnNothing（以及未声明结果）：C# 的"无返回值"即 handler 返回 null。
            return null;
        }

        private static Task<object?> ReadResource(JsonElement spec)
        {
            if (Has(spec, "return")) return Task.FromResult<object?>(Get(spec, "return"));
            if (IsSet(Get(spec, "fail")))
            {
                var f = Get(spec, "fail");
                var details = Get(f, "details");
                throw IsSet(details)
                    ? new ToolCallException(ErrorKind(Text(Get(f, "kind"))), Text(Get(f, "message")) ?? "", (object)details)
                    : new ToolCallException(ErrorKind(Text(Get(f, "kind"))), Text(Get(f, "message")) ?? "");
            }
            if (IsSet(Get(spec, "userAction")))
            {
                var u = Get(spec, "userAction");
                throw new UserActionRequiredException(Text(Get(u, "message")) ?? "", Text(Get(u, "reason")), Text(Get(u, "uri")));
            }
            throw new InvalidOperationException(Text(Get(spec, "throw")) ?? "读取失败");
        }

        /// <summary>handler 的 <c>mutate</c> 操作（conformance/README.md 2.3）。</summary>
        private void Mutate(JsonElement op)
        {
            var name = Text(Get(op, "name")) ?? "";
            switch (Text(Get(op, "op")))
            {
                case "register":
                    RegisterTool(Get(op, "tool"));
                    break;
                case "update":
                    lock (_gate)
                    {
                        var (tool, decl) = _tools[name];
                        var merged = new Dictionary<string, JsonElement>();
                        foreach (var p in decl.EnumerateObject()) merged[p.Name] = p.Value;
                        foreach (var p in Get(op, "set").EnumerateObject())
                        {
                            if (p.Value.ValueKind == JsonValueKind.Null) merged.Remove(p.Name);
                            else merged[p.Name] = p.Value;
                        }
                        var updated = JsonSerializer.SerializeToElement(merged);
                        tool.Update(updated.GetProperty("description").GetString()!, Options(updated));
                        _tools[name] = (tool, updated);
                    }
                    break;
                case "remove":
                    lock (_gate)
                    {
                        if (_tools.Remove(name, out var gone)) gone.Tool.Dispose();
                    }
                    break;
                case "enable":
                case "disable":
                    lock (_gate) _tools[name].Tool.SetEnabled(Text(Get(op, "op")) == "enable");
                    break;
                case "busy":
                    Client.SetBusy(Get(op, "value").ValueKind switch
                    {
                        JsonValueKind.True => true,
                        JsonValueKind.False => false,
                        _ => throw new InvalidOperationException("busy 的 value 应为布尔"),
                    });
                    break;
                case "declareEvent":
                    DeclareEvent(Get(op, "event"));
                    break;
                case "removeEvent":
                    Client.RemoveEvent(name);
                    break;
                default:
                    throw new InvalidOperationException("未知的 mutate 操作 " + Text(Get(op, "op")));
            }
        }

        public void Dispose()
        {
            try
            {
                Client.Stop();
            }
            catch (AppMcpException)
            {
            }
            Client.Dispose();
        }
    }
}
