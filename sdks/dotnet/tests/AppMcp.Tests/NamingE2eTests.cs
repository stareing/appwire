using System.Diagnostics;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;
using Xunit.Abstractions;

namespace AppMcp.Tests;

/// <summary>
/// 按名寻址全链路（spec/naming.md；AppMcpClientOptions.RegisterName）：<c>app-mcp-host app install</c> 登记
/// samples/Named → <c>app-mcp-host stdio --name-service</c> 作为 Hub（不用唤醒器）→ 发现不激活 → 调用触发系统激活冷启动 →
/// 宽限后通道关闭、App（由激活启动）收到 IdleExit 退出 → 再次调用再激活。
/// Linux：私有 D-Bus 会话总线（没有 dbus-daemon 时跳过）；Windows：每 App 命名管道，登记目录经 LOCALAPPDATA 指到临时目录。
/// app-mcp-host 取环境变量 APP_MCP_HOST_BIN，否则 &lt;CARGO_TARGET_DIR 或 repo/target&gt;/debug/app-mcp-host；找不到时跳过。
/// </summary>
public class NamingE2eTests(ITestOutputHelper output)
{
    private static readonly TimeSpan Timeout = TimeSpan.FromSeconds(30);

    [Fact]
    public async Task ColdActivationByNameCallGraceAndReactivation()
    {
        if (!OperatingSystem.IsLinux() && !OperatingSystem.IsWindows())
        {
            output.WriteLine("SKIP：按名寻址只在 Linux / Windows 上支持");
            return;
        }
        var hostBin = LocateHost();
        var appBin = LocateNamedSample();
        if (hostBin is null || appBin is null) return;

        var work = Directory.CreateTempSubdirectory("app-mcp-naming-cs-").FullName;
        var appId = "named-cs-" + Guid.NewGuid().ToString("N")[..8];
        var events = Path.Combine(work, "events.log");
        var dataHome = Path.Combine(work, "data");
        var home = Path.Combine(work, "home");
        Directory.CreateDirectory(dataHome);
        Directory.CreateDirectory(home);
        // 被激活的 App 读这两个变量：Linux 经 dbus-daemon 的环境传给被激活进程，Windows 经 Hub 进程的环境（exec 激活继承）。
        var appEnv = new Dictionary<string, string> { ["APP_MCP_EVENT_LOG"] = events, ["APP_MCP_APP_ID"] = appId };
        var hubEnv = new Dictionary<string, string>();
        PrivateBus? bus = null;
        try
        {
            if (OperatingSystem.IsLinux())
            {
                bus = PrivateBus.Start(work, appEnv, output);
                if (bus is null) return;
                hubEnv["DBUS_SESSION_BUS_ADDRESS"] = bus.Address;
            }
            else
            {
                foreach (var (k, v) in appEnv) hubEnv[k] = v;
                hubEnv["LOCALAPPDATA"] = dataHome; // PipeConnector 的登记目录：%LOCALAPPDATA%\app-mcp\apps
            }

            var manifest = Path.Combine(work, "manifest.json");
            File.WriteAllText(manifest, new JsonObject
            {
                ["manifestVersion"] = 1,
                ["appId"] = appId,
                ["name"] = "按名寻址 e2e（C#）",
                ["tools"] = new JsonArray(
                    new JsonObject { ["name"] = "echo", ["description"] = "原样返回参数", ["inputSchema"] = new JsonObject { ["type"] = "object" } },
                    new JsonObject { ["name"] = "pid", ["description"] = "返回进程号", ["inputSchema"] = new JsonObject { ["type"] = "object" } }),
            }.ToJsonString());
            var install = RunToEnd(hostBin, hubEnv,
                "app", "install", "--app-id", appId, "--exec", appBin, "--manifest", manifest, "--home", home, "--data-home", dataHome);
            output.WriteLine(install.Output);
            Assert.True(install.ExitCode == 0, "app install 失败：" + install.Output);

            await using var hub = McpStdio.Start(hostBin, hubEnv, output,
                "stdio", "--home", home, "--ipc-endpoint", "none", "--listen", "127.0.0.1:0", "--name-service",
                "--channel-grace-ms", "400", "--lease-ms", "0", "--waker", "none", "--tool-exposure", "all");
            await hub.RequestAsync("initialize", new JsonObject
            {
                ["protocolVersion"] = "2025-06-18",
                ["capabilities"] = new JsonObject(),
                ["clientInfo"] = new JsonObject { ["name"] = "naming-e2e-cs", ["version"] = "0" },
            });
            await hub.NotifyAsync("notifications/initialized");

            // 1. 发现不激活：apps.list 中出现 nameService（可激活、未运行），App 进程从未启动。
            JsonNode? entry = null;
            await Eventually("发现记录", async () =>
            {
                var apps = ResultData(await hub.CallToolAsync("apps.list", new JsonObject()))?["apps"]?.AsArray();
                entry = apps?.FirstOrDefault(a => (string?)a?["appId"] == appId)?["nameService"];
                return entry is JsonObject;
            });
            output.WriteLine("nameService: " + entry!.ToJsonString());
            Assert.True((bool?)entry["activatable"] == true, entry.ToJsonString());
            Assert.True((bool?)entry["running"] == false, entry.ToJsonString());
            await Task.Delay(200);
            Assert.Equal(0, Count(events, "start"));

            // 2. 调用触发激活冷启动；宽限内的第二次调用合并进同一通道。
            var first = await hub.CallToolAsync($"{appId}.echo", new JsonObject { ["x"] = 1 });
            Assert.True(ResultData(first)?["echo"]?["x"]?.GetValue<int>() == 1, first.ToJsonString());
            Assert.True((bool?)first["_meta"]?["dev.appwire/woke"] == true, first.ToJsonString());
            var pid = await hub.CallToolAsync($"{appId}.pid", new JsonObject());
            Assert.True(ResultData(pid)?["pid"] is not null, pid.ToJsonString());
            Assert.Equal(1, Count(events, "start"));

            // 3. 宽限后 Hub 关闭通道，由激活启动的 App 收到 IdleExit 退出。
            await Eventually("宽限后 App 退出", () => Task.FromResult(Count(events, "exit") == 1));

            // 4. 再次调用再激活（新进程）。
            var again = await hub.CallToolAsync($"{appId}.echo", new JsonObject { ["x"] = 2 });
            Assert.True(ResultData(again)?["echo"]?["x"]?.GetValue<int>() == 2, again.ToJsonString());
            Assert.Equal(2, Count(events, "start"));
            output.WriteLine("events:\n" + File.ReadAllText(events));
        }
        finally
        {
            KillLeftovers(events);
            bus?.Dispose();
            if (OperatingSystem.IsWindows())
            {
                RunToEnd(hostBin, hubEnv, "app", "uninstall", "--app-id", appId, "--home", home, "--data-home", dataHome);
            }
            try { Directory.Delete(work, recursive: true); } catch (IOException) { } catch (UnauthorizedAccessException) { }
        }
    }

    /// <summary>tools/call 结果的数据：structuredContent，没有时解析第一段文本。</summary>
    private static JsonNode? ResultData(JsonNode result)
    {
        if (result["structuredContent"] is JsonNode structured) return structured;
        var text = (string?)result["content"]?[0]?["text"];
        try { return text is null ? null : JsonNode.Parse(text); }
        catch (JsonException) { return null; }
    }

    private static int Count(string log, string kind) =>
        File.Exists(log) ? File.ReadAllLines(log).Count(l => l.StartsWith(kind + " ", StringComparison.Ordinal)) : 0;

    private static async Task Eventually(string what, Func<Task<bool>> condition)
    {
        var deadline = DateTime.UtcNow + Timeout;
        while (!await condition())
        {
            if (DateTime.UtcNow > deadline) throw new TimeoutException("等待超时：" + what);
            await Task.Delay(50);
        }
    }

    /// <summary>失败时结束仍在运行的被激活进程（有 start 没有 exit 的 pid）。</summary>
    private static void KillLeftovers(string log)
    {
        if (!File.Exists(log)) return;
        var lines = File.ReadAllLines(log);
        var exited = lines.Where(l => l.StartsWith("exit ", StringComparison.Ordinal)).Select(l => l[5..]).ToHashSet();
        foreach (var pid in lines.Where(l => l.StartsWith("start ", StringComparison.Ordinal)).Select(l => l[6..]).Where(p => !exited.Contains(p)))
        {
            try { using var p = Process.GetProcessById(int.Parse(pid)); p.Kill(); }
            catch (ArgumentException) { }
            catch (InvalidOperationException) { }
        }
    }

    private string? LocateHost()
    {
        var env = Environment.GetEnvironmentVariable("APP_MCP_HOST_BIN");
        if (!string.IsNullOrEmpty(env) && File.Exists(env)) return env;
        var repo = FakeHost.FindRepoRoot();
        if (repo is null)
        {
            output.WriteLine("SKIP：找不到仓库根目录");
            return null;
        }
        var targetDir = Environment.GetEnvironmentVariable("CARGO_TARGET_DIR") ?? Path.Combine(repo, "target");
        var exe = Path.Combine(targetDir, "debug", OperatingSystem.IsWindows() ? "app-mcp-host.exe" : "app-mcp-host");
        if (File.Exists(exe)) return exe;
        output.WriteLine($"SKIP：找不到 {exe}（cargo build -p app-mcp-host，或设 APP_MCP_HOST_BIN）");
        return null;
    }

    /// <summary>samples/Named 的可执行文件（测试项目引用它，构建测试时一并构建；取本测试所在源码树中最新的输出）。</summary>
    private string? LocateNamedSample()
    {
        var env = Environment.GetEnvironmentVariable("APP_MCP_NAMED_SAMPLE");
        if (!string.IsNullOrEmpty(env) && File.Exists(env)) return env;
        var dir = new DirectoryInfo(AppContext.BaseDirectory);
        while (dir is not null && !Directory.Exists(Path.Combine(dir.FullName, "samples", "Named"))) dir = dir.Parent;
        var bin = dir is null ? null : Path.Combine(dir.FullName, "samples", "Named", "bin");
        var name = OperatingSystem.IsWindows() ? "AppMcp.Samples.Named.exe" : "AppMcp.Samples.Named";
        var found = bin is not null && Directory.Exists(bin)
            ? Directory.EnumerateFiles(bin, name, SearchOption.AllDirectories).OrderByDescending(File.GetLastWriteTimeUtc).FirstOrDefault()
            : null;
        if (found is null) output.WriteLine($"SKIP：找不到 {name}（dotnet build samples/Named）");
        return found;
    }

    private static (int ExitCode, string Output) RunToEnd(string exe, IReadOnlyDictionary<string, string> env, params string[] args)
    {
        var psi = new ProcessStartInfo(exe)
        {
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            UseShellExecute = false,
            StandardOutputEncoding = Encoding.UTF8,
            StandardErrorEncoding = Encoding.UTF8,
        };
        foreach (var a in args) psi.ArgumentList.Add(a);
        foreach (var (k, v) in env) psi.Environment[k] = v;
        using var p = Process.Start(psi)!;
        var stderr = p.StandardError.ReadToEndAsync();
        var stdout = p.StandardOutput.ReadToEnd();
        if (!p.WaitForExit((int)Timeout.TotalMilliseconds)) p.Kill(entireProcessTree: true);
        return (p.ExitCode, stdout + stderr.Result);
    }

    /// <summary>临时私有 D-Bus 会话总线（Linux）：激活目录取自临时 XDG_DATA_HOME（与 app install 的 --data-home 相同）。</summary>
    private sealed class PrivateBus : IDisposable
    {
        private readonly Process _daemon;

        private PrivateBus(Process daemon, string address)
        {
            _daemon = daemon;
            Address = address;
        }

        public string Address { get; }

        public static PrivateBus? Start(string work, IReadOnlyDictionary<string, string> appEnv, ITestOutputHelper output)
        {
            var daemon = (Environment.GetEnvironmentVariable("PATH") ?? string.Empty)
                .Split(Path.PathSeparator, StringSplitOptions.RemoveEmptyEntries)
                .Select(d => Path.Combine(d, "dbus-daemon"))
                .FirstOrDefault(File.Exists);
            if (daemon is null)
            {
                output.WriteLine("SKIP：本机没有 dbus-daemon");
                return null;
            }
            foreach (var d in new[] { "data/dbus-1/services", "sys", "run" }) Directory.CreateDirectory(Path.Combine(work, d));
            var config = Path.Combine(work, "session.conf");
            File.WriteAllText(config, $"""
                <!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
                 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
                <busconfig>
                  <type>session</type>
                  <listen>unix:path={Path.Combine(work, "bus")}</listen>
                  <auth>EXTERNAL</auth>
                  <standard_session_servicedirs/>
                  <policy context="default">
                    <allow send_destination="*" eavesdrop="true"/>
                    <allow eavesdrop="true"/>
                    <allow own="*"/>
                  </policy>
                </busconfig>
                """);
            var psi = new ProcessStartInfo(daemon) { RedirectStandardOutput = true, UseShellExecute = false };
            psi.ArgumentList.Add($"--config-file={config}");
            psi.ArgumentList.Add("--nofork");
            psi.ArgumentList.Add("--print-address=1");
            psi.Environment["XDG_DATA_HOME"] = Path.Combine(work, "data");
            psi.Environment["XDG_DATA_DIRS"] = Path.Combine(work, "sys");
            psi.Environment["XDG_RUNTIME_DIR"] = Path.Combine(work, "run");
            // @why 被激活的进程继承 dbus-daemon 的环境：去掉用户会话总线地址，避免测试 App 误连用户总线。
            foreach (var k in new[] { "DBUS_SESSION_BUS_ADDRESS", "DBUS_STARTER_ADDRESS", "DBUS_STARTER_BUS_TYPE" }) psi.Environment.Remove(k);
            foreach (var (k, v) in appEnv) psi.Environment[k] = v;
            var process = Process.Start(psi)!;
            var line = process.StandardOutput.ReadLineAsync().WaitAsync(TimeSpan.FromSeconds(10)).GetAwaiter().GetResult();
            if (string.IsNullOrWhiteSpace(line))
            {
                process.Kill();
                process.Dispose();
                throw new InvalidOperationException("dbus-daemon 未打印地址");
            }
            return new PrivateBus(process, line.Trim());
        }

        public void Dispose()
        {
            try { _daemon.Kill(); _daemon.WaitForExit(5000); }
            catch (InvalidOperationException) { }
            _daemon.Dispose();
        }
    }

    /// <summary>以 stdio 运行的 MCP 服务器（app-mcp-host stdio）：每行一条 JSON-RPC 消息。</summary>
    private sealed class McpStdio : IAsyncDisposable
    {
        private readonly Process _process;
        private int _nextId;

        private McpStdio(Process process) => _process = process;

        public static McpStdio Start(string exe, IReadOnlyDictionary<string, string> env, ITestOutputHelper output, params string[] args)
        {
            var psi = new ProcessStartInfo(exe)
            {
                RedirectStandardInput = true,
                RedirectStandardOutput = true,
                RedirectStandardError = true,
                UseShellExecute = false,
                StandardInputEncoding = new UTF8Encoding(false),
                StandardOutputEncoding = Encoding.UTF8,
                StandardErrorEncoding = Encoding.UTF8,
            };
            foreach (var a in args) psi.ArgumentList.Add(a);
            foreach (var (k, v) in env) psi.Environment[k] = v;
            var p = Process.Start(psi)!;
            p.ErrorDataReceived += (_, e) =>
            {
                if (e.Data is null) return;
                try { output.WriteLine("[hub] " + e.Data); } catch (InvalidOperationException) { } // 测试结束后的日志
            };
            p.BeginErrorReadLine();
            p.StandardInput.AutoFlush = true;
            return new McpStdio(p);
        }

        public Task NotifyAsync(string method) =>
            _process.StandardInput.WriteLineAsync(new JsonObject { ["jsonrpc"] = "2.0", ["method"] = method }.ToJsonString());

        /// <summary>发送请求，返回对应 id 的 result（跳过通知）；错误响应抛出。</summary>
        public async Task<JsonNode> RequestAsync(string method, JsonObject @params)
        {
            var id = ++_nextId;
            await _process.StandardInput.WriteLineAsync(
                new JsonObject { ["jsonrpc"] = "2.0", ["id"] = id, ["method"] = method, ["params"] = @params }.ToJsonString());
            using var cts = new CancellationTokenSource(Timeout);
            while (true)
            {
                var line = await _process.StandardOutput.ReadLineAsync(cts.Token)
                    ?? throw new InvalidOperationException("Hub 已退出");
                var msg = JsonNode.Parse(line);
                if (msg?["id"] is not JsonValue v || v.GetValue<int>() != id) continue;
                return msg["result"] ?? throw new InvalidOperationException($"{method} 失败：{line}");
            }
        }

        public Task<JsonNode> CallToolAsync(string name, JsonObject arguments) =>
            RequestAsync("tools/call", new JsonObject { ["name"] = name, ["arguments"] = arguments });

        public async ValueTask DisposeAsync()
        {
            _process.StandardInput.Close(); // stdin 关闭 → Hub 退出（同时关闭仍打开的按名通道）
            using var cts = new CancellationTokenSource(TimeSpan.FromSeconds(10));
            try { await _process.WaitForExitAsync(cts.Token); }
            catch (OperationCanceledException) { _process.Kill(entireProcessTree: true); }
            _process.Dispose();
        }
    }
}
