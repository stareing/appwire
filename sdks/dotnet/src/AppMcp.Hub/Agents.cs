// Agent 身份登记（spec/hub-api.md 3.6「Agent 身份」）。
using System.Text.Json.Nodes;

namespace AppMcp.Hub;

/// <summary>一个 Agent 的访问令牌：经 MCP HTTP 出口出示此令牌的请求，主体为 agent:&lt;Name&gt;（只用于区分与归属，不做授权）。
/// <see cref="ToString"/> 不输出令牌。</summary>
public sealed class HubAgentCredential
{
    /// <summary>1–64 个 ASCII 字母、数字、-、_、.，以字母或数字开头。</summary>
    public required string Name { get; init; }
    /// <summary>32–512 个可见 ASCII 字符、不含空白（<c>Authorization: Bearer &lt;Token&gt;</c>）。</summary>
    public required string Token { get; init; }

    public override string ToString() => $"HubAgentCredential {{ Name = {Name}, Token = <redacted> }}";

    /// <summary>JSON 形式 [{"name","token"}]（hub-c 配置 agents / am_hub_set_agents）。</summary>
    internal static JsonArray ToJson(IEnumerable<HubAgentCredential> agents) =>
        new(agents.Select(a =>
        {
            ArgumentNullException.ThrowIfNull(a);
            return (JsonNode?)new JsonObject { ["name"] = a.Name, ["token"] = a.Token };
        }).ToArray());
}
