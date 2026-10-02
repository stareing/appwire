using System.Text.Json;
using System.Text.Json.Nodes;
using System.Text.Json.Serialization;

namespace AppMcp.Hub;

/// <summary>与 app_mcp_hub.h 的 AmHubStatus 一致。</summary>
public enum HubStatus
{
    Ok = 0,
    InvalidArgument = 1,
    InvalidJson = 2,
    InvalidConfig = 3,
    Io = 4,
    /// <summary>Hub 拒绝了操作；错误说明为 "&lt;KIND&gt;: &lt;message&gt;"。</summary>
    Hub = 5,
    /// <summary>审批 / 配对已超时或被取消。</summary>
    AlreadyCompleted = 6,
    Stopped = 7,
    Internal = 8,
    Panic = 9,
    /// <summary>请求的能力未编入本 Hub 库（cargo feature 关闭）；错误说明含缺少的 feature 名。此前同类错误为 <see cref="Io"/>。</summary>
    Unsupported = 10,
}

/// <summary>工具导出格式（spec/hub-api.md 第 5 节），与 AmHubToolFormat 一致。</summary>
public enum ToolFormat
{
    Mcp = 0,
    OpenAiChat = 1,
    OpenAiResponses = 2,
    Anthropic = 3,
    Gemini = 4,
}

/// <summary>风险等级（协议中的 kebab-case 字符串）。</summary>
public enum HubRisk { Read, Write, Destructive, Payment, OsSensitive }

public static class HubRiskExtensions
{
    public static string ToProtocolString(this HubRisk risk) => risk switch
    {
        HubRisk.Read => "read",
        HubRisk.Write => "write",
        HubRisk.Destructive => "destructive",
        HubRisk.Payment => "payment",
        HubRisk.OsSensitive => "os-sensitive",
        _ => throw new ArgumentOutOfRangeException(nameof(risk)),
    };
}

/// <summary>工具暴露方式（spec/hub-api.md 3.7）。</summary>
public enum ToolExposure
{
    /// <summary>App 与上游工具总数超过阈值时渐进暴露，否则全部列出（默认）。</summary>
    Auto,
    /// <summary>工具列表只含 apps.* 与本会话展开过（apps.tools）、调用过或选定了实例的 App 的工具。</summary>
    Progressive,
    /// <summary>全部列出。</summary>
    All,
}

/// <summary>MCP 出口协商的协议版本范围（spec/hub-api.md 3.6「协议版本」）。</summary>
public enum McpProtocolMode
{
    /// <summary>默认：initialize 客户端走 legacy 会话，每请求自带 _meta 的客户端可协商 2026-07-28（subscriptions/listen 可用）。</summary>
    Auto,
    /// <summary>回退开关：只声明到 2025-11-25，subscriptions/listen 不可用。</summary>
    LegacyOnly,
}

/// <summary>结果与工具 outputSchema 不符时 Hub 的处理（spec/hub-api.md 3.11）。</summary>
[JsonConverter(typeof(JsonStringEnumConverter<OutputValidation>))]
public enum OutputValidation
{
    /// <summary>不校验。</summary>
    Off,
    /// <summary>校验，不符时只记日志（默认）。</summary>
    Log,
    /// <summary>校验，不符时调用以 HANDLER_ERROR 结束。</summary>
    Reject,
}

/// <summary>
/// 资源保护（spec/hub-api.md 3.11，JSON 形式即 LimitOverrides）：按（App, 工具）与按 App 两级令牌桶限流，超出 → RATE_LIMITED；
/// 参数 / 结果 / 资源超过字节上限 → PAYLOAD_TOO_LARGE（不截断）。为 null 的字段取默认值（工具 120/分钟、突发 30；
/// App 600/分钟、突发 60；参数 1 MiB、结果 4 MiB、资源 4 MiB）。*PerMinute = 0 不限流，*Bytes = 0 不限大小；
/// 限流时 *Burst = 0 启动失败（InvalidConfig）。也用于 <see cref="HubStatusInfo.Limits"/>（全部字段给出）。
/// </summary>
public sealed class HubLimits
{
    public uint? ToolRatePerMinute { get; set; }
    public uint? ToolRateBurst { get; set; }
    public uint? AppRatePerMinute { get; set; }
    public uint? AppRateBurst { get; set; }
    public ulong? MaxArgumentsBytes { get; set; }
    public ulong? MaxResultBytes { get; set; }
    public ulong? MaxResourceBytes { get; set; }

    internal JsonObject ToJson()
    {
        var o = new JsonObject();
        if (ToolRatePerMinute is { } a) o["toolRatePerMinute"] = a;
        if (ToolRateBurst is { } b) o["toolRateBurst"] = b;
        if (AppRatePerMinute is { } c) o["appRatePerMinute"] = c;
        if (AppRateBurst is { } d) o["appRateBurst"] = d;
        if (MaxArgumentsBytes is { } e) o["maxArgumentsBytes"] = e;
        if (MaxResultBytes is { } f) o["maxResultBytes"] = f;
        if (MaxResourceBytes is { } g) o["maxResourceBytes"] = g;
        return o;
    }
}

/// <summary>策略规则的动作（spec/hub-api.md 3.13）。</summary>
[JsonConverter(typeof(JsonStringEnumConverter<HubPolicyAction>))]
public enum HubPolicyAction
{
    /// <summary>工具（或整个 App）不出现在任何列表中，调用为 TOOL_NOT_FOUND。</summary>
    Hide,
    /// <summary>可见，在 <see cref="HubPolicyRule.Hooks"/> 指定的执行点以 POLICY_DENIED 拒绝。</summary>
    Deny,
}

/// <summary>策略执行点。规则的 Hooks 只能写 Call / Wake（List 由 Hide 隐式使用，Handle 尚未实现，写了校验报错）。</summary>
[JsonConverter(typeof(JsonStringEnumConverter<HubPolicyHook>))]
public enum HubPolicyHook { List, Call, Wake, Handle }

/// <summary>按 App 声明的 MCP 注解匹配：给出的每一项都与工具注解相等才命中（工具未声明该项时不命中）。至少给出一项。</summary>
public sealed class HubAnnotationMatch
{
    public bool? ReadOnlyHint { get; set; }
    public bool? DestructiveHint { get; set; }
    public bool? IdempotentHint { get; set; }
    public bool? OpenWorldHint { get; set; }

    internal JsonObject ToJson()
    {
        var o = new JsonObject();
        if (ReadOnlyHint is { } a) o["readOnlyHint"] = a;
        if (DestructiveHint is { } b) o["destructiveHint"] = b;
        if (IdempotentHint is { } c) o["idempotentHint"] = c;
        if (OpenWorldHint is { } d) o["openWorldHint"] = d;
        return o;
    }
}

/// <summary>一条策略规则。Tool 与 Annotations 都为 null 时作用于整个 App。</summary>
public sealed class HubPolicyRule
{
    /// <summary>规则标识：[A-Za-z0-9_.-]{1,64}，在规则集中唯一；POLICY_DENIED 的 Details.ruleId。</summary>
    public required string Id { get; set; }
    public required HubPolicyAction Action { get; set; }
    /// <summary>appId（或上游名）：精确名，或以 * 结尾的前缀；"*" 匹配全部。</summary>
    public required string App { get; set; }
    /// <summary>工具局部名（不含 appId），规则同 App。</summary>
    public string? Tool { get; set; }
    public HubAnnotationMatch? Annotations { get; set; }
    /// <summary>只对该 Agent（登记的名字，规则同 App）发起的操作生效；只用于 Deny。</summary>
    public string? Agent { get; set; }
    /// <summary>只用于 Deny：Call / Wake 的非空子集；null 为 [Call]。</summary>
    public IList<HubPolicyHook>? Hooks { get; set; }

    internal static string HookString(HubPolicyHook h) => h.ToString().ToLowerInvariant();

    internal JsonObject ToJson()
    {
        var o = new JsonObject
        {
            ["id"] = Id,
            ["action"] = Action.ToString().ToLowerInvariant(),
            ["app"] = App,
        };
        if (Tool is not null) o["tool"] = Tool;
        if (Annotations is { } m) o["annotations"] = m.ToJson();
        if (Agent is not null) o["agent"] = Agent;
        if (Hooks is { } hooks) o["hooks"] = new JsonArray(hooks.Select(h => (JsonNode?)HookString(h)).ToArray());
        return o;
    }
}

/// <summary>
/// 策略规则集（spec/hub-api.md 3.13）：按顺序匹配，Deny 取第一条命中的规则；空规则集 = 不做任何限制（默认）。
/// 用于 <see cref="HubOptions.Policy"/> 与 <see cref="AppMcpHub.SetPolicy"/>。
/// </summary>
public sealed class HubPolicy
{
    public IList<HubPolicyRule> Rules { get; init; } = new List<HubPolicyRule>();

    /// <summary>JSON 形式 {"rules":[{"id","action","app","tool"?,"annotations"?,"agent"?,"hooks"?}]}。</summary>
    public string ToJsonString() => ToJson().ToJsonString();

    internal JsonObject ToJson() =>
        new() { ["rules"] = new JsonArray(Rules.Select(r => (JsonNode?)r.ToJson()).ToArray()) };
}

/// <summary>唤醒器配置（spec/hub-api.md 3.5）；<see cref="AppMcpHub"/> 的自定义唤醒回调优先。</summary>
public sealed class WakerOptions
{
    private readonly JsonNode _json;
    private WakerOptions(JsonNode json) => _json = json;

    /// <summary>按平台执行系统激活（默认）。</summary>
    public static WakerOptions System { get; } = new(JsonValue.Create("system"));
    /// <summary>不唤醒：休眠实例 / 未运行 App 的调用直接返回 APP_DISCONNECTED。</summary>
    public static WakerOptions None { get; } = new(JsonValue.Create("none"));

    /// <summary>执行 program args…（不经 shell），唤醒请求以一行 JSON 写入其 stdin。</summary>
    public static WakerOptions Exec(string program, params string[] args)
    {
        if (string.IsNullOrEmpty(program)) throw new ArgumentException("program 不能为空", nameof(program));
        var argv = new JsonArray((JsonNode?)program);
        foreach (var a in args) argv.Add(a);
        return new WakerOptions(new JsonObject { ["exec"] = argv });
    }

    internal JsonNode ToJson() => _json.DeepClone();
}

public class HubException : Exception
{
    public HubException(HubStatus status, string message) : base(message) => Status = status;
    public HubStatus Status { get; }
}
