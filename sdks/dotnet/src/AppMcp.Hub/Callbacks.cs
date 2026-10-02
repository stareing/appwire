// 回调：唤醒、事件参数、审批与配对请求（spec/hub-api.md 3.3、3.5）。
using System.Text.Json;
using System.Text.Json.Nodes;
using System.Text.Json.Serialization;

namespace AppMcp.Hub;

/// <summary>唤醒请求（WakeRequest，spec/hub-api.md 3.5），交给 <see cref="AppMcpHub.Waker"/>。</summary>
/// <param name="AppId">要唤醒的 App。</param>
/// <param name="InstanceId">被唤醒的休眠实例；null = App 未运行，按清单冷启动。</param>
/// <param name="Descriptor">唤醒描述：{kind, target?, background}。</param>
/// <param name="Token">一次性唤醒令牌（32 位十六进制）。</param>
/// <param name="ActivationArg">通用激活参数 <c>app-mcp-wake:&lt;token&gt;</c>，App 端 SDK 的 HandleWake 可识别。</param>
public sealed record WakeRequest(
    string AppId,
    string? InstanceId,
    WakeTarget Descriptor,
    string Token,
    string ActivationArg);

/// <summary>唤醒描述。<see cref="Kind"/> 为 uri / aumid / apple-event / dbus / android-intent / web-url。</summary>
public sealed record WakeTarget(string Kind, string? Target, bool Background);

/// <summary>在 <see cref="AppMcpHub.Waker"/> 中抛出，以指定错误类别结束调用（其他异常按 LAUNCH_FAILED）。</summary>
public class WakeFailedException(string kind, string message) : Exception(message)
{
    /// <summary>协议错误类别，如 LAUNCH_FAILED、APP_NOT_INSTALLED。</summary>
    public string Kind { get; } = kind;
}

/// <summary>Hub 事件。<see cref="Type"/> 为 appConnected、toolsChanged 等（spec/hub-api.md 3.1）；
/// 回调处理过慢导致事件丢失时为 "lagged"，此时应重新查询全量状态。
/// 未来新增的事件类型原样透传，调用方应忽略不认识的类型。</summary>
public sealed class HubEventArgs(string type, JsonElement json) : EventArgs
{
    public string Type { get; } = type;
    /// <summary>完整事件 JSON（含 type）。</summary>
    public JsonElement Json { get; } = json;

    public string? AppId => Str("appId");
    public string? InstanceId => Str("instanceId");
    public string? Uri => Str("uri");

    private string? Str(string name) =>
        Json.TryGetProperty(name, out var v) && v.ValueKind == JsonValueKind.String ? v.GetString() : null;
}

/// <summary>调用审批请求（ApprovalRequest）。</summary>
public sealed record ApprovalRequest(
    string CallId,
    string AppId,
    string AppName,
    string Tool,
    string? Title,
    string Description,
    string Risk,
    JsonElement Arguments,
    string? Session)
{
    /// <summary>与 <see cref="HubToolInfo.Annotations"/> 相同；旧 Hub 为 null。</summary>
    public HubToolAnnotations? Annotations { get; init; }
    /// <summary>MCP 出口：发起调用的认证主体（取自传输层凭据，现在恒为 "local"）；经 Hub API 发起时为 null。</summary>
    public string? Principal { get; init; }
    /// <summary>MCP 出口：客户端自报的 clientInfo.name；经 Hub API 发起时为 null。自报、不可信，仅供显示，不得据此做授权决定。</summary>
    public string? ClientName { get; init; }
}

/// <summary>App 配对请求（PairingRequest）。</summary>
public sealed record PairingRequest(
    string AppId,
    string AppName,
    string? Origin,
    string ClientKind,
    string InstanceId);
