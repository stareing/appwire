namespace AppMcp.Hub.Tests;

/// <summary>
/// 工具演进（第 16 项 O4，spec/hub-api.md 3.21）：真实 App 经 C ABI v23 声明 deprecated → <see cref="HubToolInfo.Deprecated"/> 原样、
/// <see cref="HubToolInfo.SchemaHash"/> 为 16 位十六进制；App 改成不兼容定义后 schemaHash 变化、Status().SchemaChanges 出现一条 breaking。
/// </summary>
public class EvolutionTests
{
    private const string Schema = """{"type":"object","properties":{"q":{"type":"string"},"page":{"type":"integer"}}}""";
    private const string BreakingSchema = """{"type":"object","properties":{"q":{"type":"string"},"page":{"type":"integer"}},"required":["page"]}""";

    [Fact]
    public async Task DeprecatedAndSchemaHashAndSchemaChanges()
    {
        await using var hub = AppMcpHub.Start(new HubOptions { Listen = "127.0.0.1:0", DisableIpc = true, Dispatcher = null });
        await using var client = AppMcp.AppMcpClient.Create(new AppMcp.AppMcpClientOptions
        {
            AppId = "orders", AppName = "订单", HostUrl = $"ws://{hub.ListenAddress}/app", Dispatcher = null,
        });
        Task<object?> Handler(System.Text.Json.JsonElement _, AppMcp.ToolContext __) => Task.FromResult<object?>(null);
        using var list = client.RegisterTool("list", "旧版列表", Handler, new AppMcp.ToolOptions
        {
            InputSchemaJson = Schema,
            Deprecated = new AppMcp.ToolDeprecation("改用 orders.list2", "list2", "2027-06-30"),
        });
        using var list2 = client.RegisterTool("list2", "新版列表", Handler);
        client.Start();
        await ResultCacheTests.WaitTool(hub, "orders.list");
        await ResultCacheTests.WaitTool(hub, "orders.list2");

        var old = hub.ListTools().Single(t => t.Name == "orders.list");
        Assert.Equal(new ToolDeprecationInfo("改用 orders.list2", "list2", "2027-06-30"), old.Deprecated);
        Assert.Matches("^[0-9a-f]{16}$", old.SchemaHash);
        var plain = hub.ListTools().Single(t => t.Name == "orders.list2");
        Assert.Null(plain.Deprecated);
        Assert.NotNull(plain.SchemaHash);
        Assert.NotEqual(old.SchemaHash, plain.SchemaHash);
        Assert.Empty(hub.Status().SchemaChanges!);

        // 新增必填参数（不兼容）并清除弃用
        list.Update("旧版列表", new AppMcp.ToolOptions { InputSchemaJson = BreakingSchema });
        var deadline = DateTime.UtcNow + TimeSpan.FromSeconds(10);
        HubToolInfo updated;
        while ((updated = hub.ListTools().Single(t => t.Name == "orders.list")).SchemaHash == old.SchemaHash)
        {
            if (DateTime.UtcNow > deadline) throw new TimeoutException("等待工具更新超时");
            await Task.Delay(20);
        }
        Assert.Null(updated.Deprecated);
        var record = Assert.Single(hub.Status().SchemaChanges!);
        Assert.Equal(("orders", "list", "breaking"), (record.AppId, record.Tool, record.Level));
        Assert.NotEmpty(record.Changes);
        Assert.True(record.At > 0);
    }
}
