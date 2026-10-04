// verify.sh 的 deprecated 步骤：实现 deprecated.json 生成的 ILegacyToolHandlers（实现已弃用的成员不产生警告）并分派。
using System;
using System.Text.Json;
using System.Threading;
using System.Threading.Tasks;
using AppMcp.Generated.Legacy;

internal sealed class Impl : ILegacyToolHandlers
{
    public Task<object?> OrdersListAsync(OrdersListParams args, CancellationToken ct) => Task.FromResult<object?>(args);
    public Task<object?> OrdersFindAsync(OrdersFindParams args, CancellationToken ct) => Task.FromResult<object?>(args);
    public Task<object?> CartLegacyClearAsync(CartLegacyClearParams args, CancellationToken ct) => Task.FromResult<object?>("cleared");
}

internal static class Program
{
    private static async Task<int> Main()
    {
        var list = await LegacyTools.DispatchAsync(new Impl(), "orders.list", JsonDocument.Parse("""{"status":"paid","page":2,"limit":3}""").RootElement);
        var json = JsonSerializer.Serialize(list, LegacyTools.JsonOptions);
        Check(json.Contains("\"status\":\"paid\"") && json.Contains("\"page\":2"), "弃用属性往返：" + json);
        var find = (OrdersFindParams)(await LegacyTools.DispatchAsync(new Impl(), "orders.find", JsonDocument.Parse("""{"state":"s","filter":{"legacyTag":"t"}}""").RootElement))!;
        Check(JsonSerializer.Serialize(find, LegacyTools.JsonOptions).Contains("\"legacyTag\":\"t\""), "嵌套弃用属性");
        Check((string?)await LegacyTools.DispatchAsync(new Impl(), "cart.legacyClear", default) == "cleared", "弃用工具分派");
        Console.WriteLine("ok");
        return 0;
    }

    private static void Check(bool ok, string what)
    {
        if (!ok) throw new InvalidOperationException("检查失败：" + what);
    }
}
