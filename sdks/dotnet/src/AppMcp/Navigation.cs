using System.Text.Json;

namespace AppMcp;

/// <summary>一次导航请求（Host 的 <c>app/navigate</c>，spec/protocol.md 3.4）。</summary>
/// <param name="Page">目标页面名（清单 <c>pages[].name</c> 或工具的 <see cref="ToolOptions.Page"/>）。</param>
/// <param name="ParamsJson">页面参数 JSON 文本；Host 没有给出时为 null。</param>
public sealed record NavigationRequest(string Page, string? ParamsJson)
{
    /// <summary>页面参数；Host 没有给出时为 null。</summary>
    public JsonElement? Params => ParamsJson is null ? null : JsonDocument.Parse(ParamsJson).RootElement.Clone();
}

/// <summary>在导航回调中抛出：拒绝本次导航（NAVIGATION_DENIED，如用户正在输入、页面需要登录）。消息面向模型 / 用户。</summary>
public class NavigationDeniedException(string message) : Exception(message);
