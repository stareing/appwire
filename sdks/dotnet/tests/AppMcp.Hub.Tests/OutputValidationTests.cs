using System.Text.Json;

namespace AppMcp.Hub.Tests;

/// <summary>经 C ABI 的 outputValidation = reject / log 与 PAYLOAD_TOO_LARGE（第 14、19 项）：结果不符 outputSchema 时的端到端行为。</summary>
public class OutputValidationBindingTests
{
    private static readonly TimeSpan Wait = TimeSpan.FromSeconds(10);
    private const string OrderSchema = """{"type":"object","properties":{"orderId":{"type":"string"}},"required":["orderId"]}""";

    private static async Task<(AppMcpHub Hub, AppMcp.AppMcpClient App)> Start(OutputValidation mode, HubLimits? limits = null)
    {
        var hub = AppMcpHub.Start(new HubOptions
        {
            DisableIpc = true,
            Listen = "127.0.0.1:0",
            Dispatcher = null,
            OutputValidation = mode,
            Limits = limits,
        });
        var app = AppMcp.AppMcpClient.Create(new AppMcp.AppMcpClientOptions
        {
            AppId = "shop",
            AppName = "商店",
            HostUrl = $"ws://{hub.ListenAddress}/app",
            Dispatcher = null,
        });
        var schema = new AppMcp.ToolOptions { OutputSchemaJson = OrderSchema };
        // orderId 为数字：不符合 schema
        app.RegisterTool("order.bad", "坏结果", (_, _) => Task.FromResult<object?>(new { orderId = 42 }), schema);
        app.RegisterTool("order.good", "好结果", (_, _) => Task.FromResult<object?>(new { orderId = "o1" }), schema);
        // 无返回值：data 为 null，不校验
        app.RegisterTool("order.none", "无返回值", (_, _) => Task.FromResult<object?>(null), schema);
        app.RegisterTool("blob", "大结果", (_, _) => Task.FromResult<object?>(new { s = new string('x', 4096) }));
        app.Start();
        var deadline = DateTime.UtcNow + Wait;
        while (hub.ListTools(new ToolFilter { Apps = ["shop"], OnlyAvailable = true, IncludeBuiltin = false }).Count < 4)
        {
            if (DateTime.UtcNow > deadline) throw new TimeoutException("工具未同步");
            await Task.Delay(20);
        }
        return (hub, app);
    }

    [Fact]
    public async Task RejectModeFailsMismatchedResult()
    {
        var (hub, app) = await Start(OutputValidation.Reject);
        await using var _h = hub;
        await using var _a = app;

        var bad = await hub.CallAsync("shop.order.bad");
        Assert.False(bad.IsSuccess, bad.Json.GetRawText());
        Assert.Equal("HANDLER_ERROR", bad.Error?.Kind);
        Assert.Equal("shop", bad.Error!.Details!.Value.GetProperty("appId").GetString());
        Assert.False(string.IsNullOrEmpty(bad.Error.Details.Value.GetProperty("outputSchemaError").GetString()));

        var good = await hub.CallAsync("shop.order.good");
        Assert.True(good.IsSuccess, good.Json.GetRawText());
        Assert.Equal("o1", good.Data!.Value.GetProperty("orderId").GetString());

        var none = await hub.CallAsync("shop.order.none");
        Assert.True(none.IsSuccess, none.Json.GetRawText());
    }

    [Fact]
    public async Task LogModePassesMismatchedResult()
    {
        var (hub, app) = await Start(OutputValidation.Log);
        await using var _h = hub;
        await using var _a = app;
        var bad = await hub.CallAsync("shop.order.bad");
        Assert.True(bad.IsSuccess, bad.Json.GetRawText());
        Assert.Equal(42, bad.Data!.Value.GetProperty("orderId").GetInt32());
    }

    [Fact]
    public async Task ResultOverLimitIsPayloadTooLarge()
    {
        var (hub, app) = await Start(OutputValidation.Log, new HubLimits { MaxResultBytes = 1024, MaxArgumentsBytes = 64 });
        await using var _h = hub;
        await using var _a = app;

        var big = await hub.CallAsync("shop.blob");
        Assert.Equal(HubError.PayloadTooLarge, big.Error?.Kind);
        Assert.Equal("result", big.Error!.Details!.Value.GetProperty("part").GetString());

        var args = await hub.CallAsync("shop.order.good", new { pad = new string('y', 200) });
        Assert.Equal(HubError.PayloadTooLarge, args.Error?.Kind);
        Assert.Equal("arguments", args.Error!.Details!.Value.GetProperty("part").GetString());

        Assert.True((await hub.CallAsync("shop.order.good")).IsSuccess);
        Assert.Equal(2UL, Assert.Single(hub.Status().Apps, a => a.AppId == "shop").TooLarge);
    }
}
