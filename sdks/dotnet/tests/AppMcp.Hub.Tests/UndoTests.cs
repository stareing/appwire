namespace AppMcp.Hub.Tests;

/// <summary>
/// 撤销（第 15 项 X2，spec/hub-api.md 3.23）：真实 App 经 C ABI v24 声明 undoable、handler 返回 <see cref="AppMcp.ToolResult.Undo"/> →
/// HubToolInfo.Undoable、CallOutcome.Undo；apps.undo 得到逆调用结果与 UndoOf；Status().Undo；HubOptions.Undo.MaxPerTask = 0 关闭撤销。
/// </summary>
public class UndoTests
{
    /// <summary>注册 todo.add（undoable，结果带逆操作 todo.remove）与 todo.remove 的 App。</summary>
    private sealed class TodoApp : IAsyncDisposable
    {
        private readonly AppMcp.AppMcpClient _client;
        private readonly AppMcp.ToolRegistration _add;
        private readonly AppMcp.ToolRegistration _remove;

        public TodoApp(AppMcpHub hub)
        {
            _client = AppMcp.AppMcpClient.Create(new AppMcp.AppMcpClientOptions
            {
                AppId = "todo",
                AppName = "待办",
                HostUrl = $"ws://{hub.ListenAddress}/app",
                Dispatcher = null,
            });
            _add = _client.RegisterTool("add", "添加待办",
                (_, _) => Task.FromResult<object?>(new AppMcp.ToolResult(new { id = 3 })
                {
                    Undo = new AppMcp.UndoAction("remove", new { id = 3 }, "删除刚添加的待办"),
                }),
                new AppMcp.ToolOptions { Undoable = true });
            _remove = _client.RegisterTool("remove", "删除待办",
                (args, _) => Task.FromResult<object?>(new { removed = args.GetProperty("id").GetInt32() }));
            _client.Start();
        }

        public async ValueTask DisposeAsync()
        {
            _add.Dispose();
            _remove.Dispose();
            await _client.DisposeAsync();
        }
    }

    [Fact]
    public async Task UndoableOutcomeAppsUndoAndStatus()
    {
        await using var hub = AppMcpHub.Start(new HubOptions
        {
            Listen = "127.0.0.1:0",
            DisableIpc = true,
            Dispatcher = null,
            Undo = new HubUndoLimits { Ttl = TimeSpan.FromMinutes(1), MaxPerTask = 4 },
        });
        await using var app = new TodoApp(hub);
        await ResultCacheTests.WaitTool(hub, "todo.add");
        await ResultCacheTests.WaitTool(hub, "todo.remove");
        Assert.True(hub.ListTools().Single(t => t.Name == "todo.add").Undoable);
        Assert.False(hub.ListTools().Single(t => t.Name == "todo.remove").Undoable);
        Assert.Equal(new UndoStatusInfo(60_000, 4, 0), hub.Status().Undo);

        var added = await hub.CallAsync(new CallRequest("todo.add"));
        Assert.True(added.IsSuccess, added.Json.GetRawText());
        Assert.Equal(3, added.Data!.Value.GetProperty("id").GetInt32());
        Assert.Equal("删除刚添加的待办", added.Undo?.Label);
        Assert.InRange(added.Undo!.ExpiresInMs, 1UL, 60_000UL);
        Assert.Null(added.UndoOf);
        Assert.Equal(1, hub.Status().Undo!.Records);

        var undone = await hub.CallAsync(new CallRequest("apps.undo", new { callId = added.CallId }));
        Assert.True(undone.IsSuccess, undone.Json.GetRawText());
        Assert.Equal(3, undone.Data!.Value.GetProperty("removed").GetInt32());
        Assert.Equal(added.CallId, undone.UndoOf);
        Assert.Null(undone.Undo);
        var again = await hub.CallAsync(new CallRequest("apps.undo", new { callId = added.CallId }));
        Assert.Equal("TOOL_NOT_FOUND", again.Error?.Kind);
    }

    [Fact]
    public async Task MaxPerTaskZeroDisablesUndo()
    {
        await using var hub = AppMcpHub.Start(new HubOptions
        {
            Listen = "127.0.0.1:0",
            DisableIpc = true,
            Dispatcher = null,
            Undo = new HubUndoLimits { MaxPerTask = 0 },
        });
        await using var app = new TodoApp(hub);
        await ResultCacheTests.WaitTool(hub, "todo.add");
        Assert.DoesNotContain(hub.ListTools(), t => t.Name == "apps.undo");
        var added = await hub.CallAsync(new CallRequest("todo.add"));
        Assert.True(added.IsSuccess, added.Json.GetRawText());
        Assert.Null(added.Undo);
        Assert.Equal(0, hub.Status().Undo!.MaxPerTask);
        Assert.Throws<ArgumentOutOfRangeException>(() => AppMcpHub.Start(new HubOptions
        {
            Listen = null, DisableIpc = true, Dispatcher = null, Undo = new HubUndoLimits { MaxPerTask = -1 },
        }));
    }
}
