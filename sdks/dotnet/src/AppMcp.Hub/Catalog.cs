// App 与工具目录（AppInfo、HubToolInfo）。
using System.Text.Json;
using System.Text.Json.Nodes;
using System.Text.Json.Serialization;

namespace AppMcp.Hub;

/// <summary>App 信息（AppInfo）。</summary>
public sealed record AppInfo(
    string AppId,
    string Name,
    string Kind,
    string? Summary,
    bool Connected,
    IReadOnlyList<InstanceInfo> Instances,
    string? SelectedInstance,
    IReadOnlyList<InstanceInfo>? DormantInstances)
{
    /// <summary>休眠中的实例（按休眠时间排列）；旧版本库不返回时为空。</summary>
    public IReadOnlyList<InstanceInfo> DormantInstances { get; init; } = DormantInstances ?? [];

    /// <summary>无已连接实例、但有休眠实例（调用其工具时会唤醒）。</summary>
    public bool IsDormant => !Connected && DormantInstances.Count > 0;
}

/// <summary>工具信息（HubTool）。</summary>
public sealed record HubToolInfo(
    string Name,
    string AppId,
    string Tool,
    string? Title,
    string Description,
    JsonElement InputSchema,
    string Risk,
    string Activation,
    string Availability)
{
    public bool IsDormant => Availability == HubAvailability.Dormant;
    /// <summary>Agent 实际看到的 MCP 工具注解（声明优先、缺少的按 Risk 推导；上游为原样）；旧 Hub 为 null。</summary>
    public HubToolAnnotations? Annotations { get; init; }
    /// <summary>App 声明的结果 JSON Schema（MCP outputSchema）；未声明为 null。</summary>
    public JsonElement? OutputSchema { get; init; }
    /// <summary>App 工具的界面依赖（<see cref="HubToolSurface"/> 的取值）；内置与上游工具为 null。</summary>
    public string? Surface { get; init; }
    /// <summary>App 工具所在页面（spec/hub-api.md 3.14）；不属于页面时为 null。</summary>
    public string? Page { get; init; }
    /// <summary>App 声明实现的标准意图（spec/intents.md，如 <c>message.send@1</c>）；未声明为 null。</summary>
    public IReadOnlyList<string>? Implements { get; init; }
    /// <summary>App 工具定义的 schemaHash（spec/hub-api.md 3.21）：inputSchema / outputSchema 变化时随之变化；内置与上游工具为 null。</summary>
    public string? SchemaHash { get; init; }
    /// <summary>App 工具的弃用声明；未弃用、内置与上游工具为 null。</summary>
    public ToolDeprecationInfo? Deprecated { get; init; }
}
