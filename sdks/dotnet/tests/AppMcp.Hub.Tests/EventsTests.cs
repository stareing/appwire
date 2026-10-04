using System.Collections.Concurrent;
using System.Text.Json;
using System.Text.Json.Nodes;

namespace AppMcp.Hub.Tests;

/// <summary>
/// 事件、订阅与信箱（第 16 项 N3 + P4，spec/hub-api.md 3.17）：真实 App 端 EmitEvent → Agent 会话 apps.events.subscribe / apps.events 取件、
/// HubStatusInfo.Events 计数、SetEventHandler 与 Event（appEvent）回调。
/// </summary>
public class EventsTests
{
    private static readonly TimeSpan Wait = TimeSpan.FromSeconds(10);

    private static async Task<T> WaitFor<T>(Func<T?> probe, string what) where T : class
    {
        var deadline = DateTime.UtcNow + Wait;
        for (;;)
        {
            if (probe() is { } v) return v;
            if (DateTime.UtcNow > deadline) throw new TimeoutException($"等待超时：{what}");
            await Task.Delay(20);
        }
    }

    [Fact]
    public void BuiltinsListedAndStatusEmpty()
    {
        using var hub = AppMcpHub.Start(new HubOptions { DisableIpc = true, DisableListen = true, Dispatcher = null });
        var names = hub.ListTools().Select(t => t.Name).ToArray();
        Assert.Contains("apps.events.subscribe", names);
        Assert.Contains("apps.events.unsubscribe", names);
        Assert.Contains("apps.events", names);
        var events = hub.Status().Events!;
        Assert.Empty(events.Subscriptions);
        Assert.Equal(0UL, events.DroppedInvalid);
    }

    [Fact]
    public async Task AppEventReachesInboxStatusAndHandler()
    {
        await using var hub = AppMcpHub.Start(new HubOptions { Listen = "127.0.0.1:0", DisableIpc = true, Dispatcher = null });
        var handled = new BlockingCollection<HubAppEvent>();
        var streamed = new BlockingCollection<HubEventArgs>();
        hub.SetEventHandler(handled.Add);
        hub.Event += (_, e) => { if (e.Type == HubEventTypes.AppEvent) streamed.Add(e); };

        await using var app = AppMcp.AppMcpClient.Create(new AppMcp.AppMcpClientOptions
        {
            AppId = "orders",
            AppName = "订单",
            HostUrl = $"ws://{hub.ListenAddress}/app",
            Dispatcher = null,
        });
        app.DeclareEvent("order.shipped", "订单已发货", """{"type":"object","properties":{"orderId":{"type":"string"}}}""");
        app.Start();
        await WaitFor(() => hub.ListApps().Any(a => a.AppId == "orders") ? "" : null, "App 连接");

        // 订阅：未声明的事件 → INVALID_INPUT；按事件名订阅。
        var bad = await hub.CallAsync(new CallRequest("apps.events.subscribe", new { appId = "orders", @event = "nope" }) { Session = "s1" });
        Assert.Equal("INVALID_INPUT", bad.Error?.Kind);
        var sub = await hub.CallAsync(new CallRequest("apps.events.subscribe", new { appId = "orders", @event = "order.shipped" }) { Session = "s1" });
        Assert.True(sub.IsSuccess, sub.Json.GetRawText());
        var subscriptionId = sub.Data!.Value.GetProperty("subscriptionId").GetString()!;

        // 未连接时 false；已连接后 true（握手可能尚未完成，重试到发出）。
        await WaitFor(() => app.EmitEvent("order.shipped", new { orderId = "o1" }) ? "" : null, "事件发出");

        Assert.True(handled.TryTake(out var ev, Wait), "SetEventHandler 未收到事件");
        Assert.Equal(("orders", "order.shipped"), (ev!.AppId, ev.Name));
        Assert.StartsWith("ev-", ev.Id);
        Assert.False(string.IsNullOrEmpty(ev.InstanceId));
        Assert.True(ev.At > 0);
        Assert.Equal("o1", ev.Payload!.Value.GetProperty("orderId").GetString());
        Assert.True(streamed.TryTake(out var se, Wait), "Event 未收到 appEvent");
        Assert.Equal(ev.Id, se!.Json.GetProperty("id").GetString());

        // 状态：一个订阅，已投递 1、积压 1。
        var status = hub.Status().Events!;
        var s = Assert.Single(status.Subscriptions);
        Assert.Equal((subscriptionId, "api:s1", "orders", "order.shipped", 1UL, 0UL, 1),
            (s.SubscriptionId, s.Subscriber, s.AppId, s.Event, s.Delivered, s.Dropped, s.Pending));

        // 其他会话取不到；订阅方取件后移出信箱。
        var other = await hub.CallAsync(new CallRequest("apps.events") { Session = "s2" });
        Assert.Equal(0, other.Data!.Value.GetProperty("events").GetArrayLength());
        var fetched = await hub.CallAsync(new CallRequest("apps.events") { Session = "s1" });
        var events = fetched.Data!.Value.GetProperty("events");
        var got = Assert.Single(events.EnumerateArray()).Deserialize<HubAppEvent>(new JsonSerializerOptions(JsonSerializerDefaults.Web))!;
        Assert.Equal((ev.Id, ev.Name), (got.Id, got.Name));
        Assert.Equal(0, fetched.Data!.Value.GetProperty("pending").GetInt32());
        Assert.Equal(0, Assert.Single(hub.Status().Events!.Subscriptions).Pending);

        // 清除厂商回调：之后的事件不再回调（Event 仍收到）。
        hub.SetEventHandler(null);
        Assert.True(app.EmitEvent("order.shipped"));
        Assert.True(streamed.TryTake(out var second, Wait));
        Assert.False(second!.Json.TryGetProperty("payload", out _));
        Assert.False(handled.TryTake(out _, TimeSpan.FromMilliseconds(200)), "清除后仍收到回调");

        var unsub = await hub.CallAsync(new CallRequest("apps.events.unsubscribe", new { subscriptionId }) { Session = "s1" });
        Assert.True(unsub.IsSuccess, unsub.Json.GetRawText());
        Assert.Empty(hub.Status().Events!.Subscriptions);
        app.Stop();
    }

    [Fact]
    public void EventLimitsSerialize()
    {
        Assert.False(JsonNode.Parse(new HubOptions().ToConfigJson())!.AsObject().ContainsKey("eventLimits"));
        var json = new HubOptions
        {
            EventLimits = new HubEventLimits { MaxInboxEvents = 5, InboxTtl = TimeSpan.FromSeconds(2), PerSubscriptionPerMinute = 0 },
        }.ToConfigJson();
        var el = JsonNode.Parse(json)!["eventLimits"]!.AsObject();
        Assert.Equal(5, (int?)el["maxInboxEvents"]);
        Assert.Equal(2000UL, (ulong?)el["inboxTtlMs"]);
        Assert.Equal(0, (int?)el["perSubscriptionPerMinute"]);
        Assert.False(el.ContainsKey("maxSubscriptions"), "未设置的字段不写出");
        Assert.Throws<ArgumentOutOfRangeException>(() => new HubOptions { EventLimits = new HubEventLimits { MaxSubscriptions = -1 } }.ToConfigJson());
    }

    /// <summary>EventLimits.MaxSubscriptions = 1：第二个订阅 → RATE_LIMITED（details.scope = "events"）。</summary>
    [Fact]
    public async Task EventLimitsMaxSubscriptionsApplies()
    {
        var options = new HubOptions
        {
            DisableIpc = true,
            DisableListen = true,
            Dispatcher = null,
            EventLimits = new HubEventLimits { MaxSubscriptions = 1 },
        };
        options.Manifests.Add(JsonNode.Parse("""{"manifestVersion":1,"appId":"shop","name":"商城","tools":[]}""")!);
        using var hub = AppMcpHub.Start(options);
        var first = await hub.CallAsync(new CallRequest("apps.events.subscribe", new { appId = "shop", @event = "a" }) { Session = "s1" });
        Assert.True(first.IsSuccess, first.Json.GetRawText());
        var second = await hub.CallAsync(new CallRequest("apps.events.subscribe", new { appId = "shop", @event = "b" }) { Session = "s1" });
        Assert.Equal(HubError.RateLimited, second.Error?.Kind);
        Assert.Equal("events", second.Error!.Details!.Value.GetProperty("scope").GetString());
    }
}
