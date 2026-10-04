using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Text.Json;
using AppMcp.Internal;
using AppMcp.Native;

namespace AppMcp.Tests;

/// <summary>撤销（spec/protocol.md 3.8，C ABI v24 <c>undoable</c> 与 <c>undo_tool</c> / <c>undo_arguments_json</c> / <c>undo_label</c>）。
/// 结果中的撤销信息到达 Host（含不合法时由核心去掉）由一致性用例 result-undo 覆盖。</summary>
public class UndoTests
{
    [Fact]
    public void CallResultUsesV24Layout()
    {
        // 与 bindings/c 的 v24_layout 一致；32 位平台的对齐随 ABI 而异，只核对 64 位布局。
        if (IntPtr.Size != 8) return;
        Assert.Equal(88, Unsafe.SizeOf<AmCallResult>());
        Assert.Equal((64, 72, 80), ((int)Marshal.OffsetOf<AmCallResult>(nameof(AmCallResult.UndoTool)),
            (int)Marshal.OffsetOf<AmCallResult>(nameof(AmCallResult.UndoArgumentsJson)),
            (int)Marshal.OffsetOf<AmCallResult>(nameof(AmCallResult.UndoLabel))));
    }

    [Fact]
    public void UndoableIsPassedAsCBool()
    {
        using var strings = new Utf8Strings();
        Assert.Equal(1, ToolScope.BuildOptions(strings, new ToolOptions { Undoable = true }).Undoable);
        Assert.Equal(0, ToolScope.BuildOptions(strings, new ToolOptions()).Undoable);
    }

    [Fact]
    public void UndoableAffectsToolsHashAndFalseClears()
    {
        using var client = AppMcpClient.Create(new AppMcpClientOptions
        {
            AppId = "dotnet-undo", AppName = "Undo", HostUrl = "ws://127.0.0.1:1", Dispatcher = null,
        });
        using var tool = client.RegisterTool("todo.add", "添加待办", (_, _) => Task.FromResult<object?>(null));
        var plain = client.ToolsHash;
        tool.Update("添加待办", new ToolOptions { Undoable = true });
        Assert.NotEqual(plain, client.ToolsHash);
        tool.Update("添加待办", new ToolOptions());
        Assert.Equal(plain, client.ToolsHash);
    }

    [Fact]
    public void UndoArgumentsAreSerializedWithClientOptions()
    {
        var json = new JsonSerializerOptions { PropertyNamingPolicy = JsonNamingPolicy.CamelCase };
        var full = ToolOutcome.From(new ToolResult(new { Id = 3 }) { Undo = new UndoAction("todo.remove", new { TodoId = 3 }, "删除") }, json);
        Assert.Equal(("{\"id\":3}", "{\"todoId\":3}"), (full.DataJson, full.UndoArgumentsJson));
        Assert.Equal("todo.remove", full.Structured?.Undo?.Tool);
        var minimal = ToolOutcome.From(new ToolResult { Undo = new UndoAction("todo.toggle") }, json);
        Assert.Null(minimal.UndoArgumentsJson); // null = {}
        Assert.Null(ToolOutcome.From(new ToolResult(1), json).UndoArgumentsJson);
    }
}
