// 事件、订阅与信箱（第 16 项 N3 + P4，spec/hub-api.md 3.17）：App 事件对象与 HubStatusInfo.Events。
using System.Text.Json;

namespace AppMcp.Hub;

/// <summary>
/// App 发出、经 Hub 去重与校验的一个事件（AppEvent），交给 <see cref="AppMcpHub.SetEventHandler"/>；
/// 内置工具 <c>apps.events</c> 取件时也是这个形状。
/// </summary>
/// <param name="Id">Hub 分配：<c>ev-&lt;n&gt;</c>。</param>
/// <param name="AppId">发出事件的 App。</param>
/// <param name="InstanceId">发出事件的实例。</param>
/// <param name="Name">事件名（App 内的局部名，如 <c>order.shipped</c>）。</param>
/// <param name="Payload">载荷（JSON 对象）；App 未给出时为 null。</param>
/// <param name="At">Hub 收到时的 Unix 毫秒。</param>
public sealed record HubAppEvent(string Id, string AppId, string InstanceId, string Name, JsonElement? Payload, ulong At);

/// <summary>事件订阅与丢弃统计（HubStatus.events）。DroppedInvalid：因未声明、载荷不合法或超限而丢弃的事件数（启动以来）。</summary>
public sealed record EventsStatusInfo(IReadOnlyList<EventSubscriptionStatusInfo> Subscriptions, ulong DroppedInvalid);

/// <summary>
/// 一个事件订阅。Subscriber：<c>agent:&lt;名&gt;</c> 或调用方键（<c>mcp:&lt;n&gt;</c> / <c>principal:local</c> / <c>api:&lt;会话&gt;</c> 等）；
/// Event 为 null 表示该 App 的全部事件；Delivered：经本订阅入箱数；Dropped：因频率上限丢弃数；Pending：订阅方信箱当前条数。
/// </summary>
public sealed record EventSubscriptionStatusInfo(
    string SubscriptionId,
    string Subscriber,
    string AppId,
    string? Event,
    ulong Delivered,
    ulong Dropped,
    int Pending);
