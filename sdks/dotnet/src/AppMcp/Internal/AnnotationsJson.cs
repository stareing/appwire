using System.Text.Encodings.Web;
using System.Text.Json;
using System.Text.Json.Serialization;

namespace AppMcp.Internal;

/// <summary>工具注解 / 内容注解 → 协议 JSON（camelCase 字段名与枚举值、省略 null）。</summary>
/// <remarks>@why 与客户端的 <see cref="AppMcpClientOptions.SerializerOptions"/> 无关：协议字段名固定，不随 App 的命名策略变化。</remarks>
internal static class AnnotationsJson
{
    private static readonly JsonSerializerOptions Options = new()
    {
        DefaultIgnoreCondition = JsonIgnoreCondition.WhenWritingNull,
        // @why 只交给本库解析（不嵌入 HTML），不必转义非 ASCII 字符，日志中保持可读。
        Encoder = JavaScriptEncoder.UnsafeRelaxedJsonEscaping,
        Converters = { new JsonStringEnumConverter(JsonNamingPolicy.CamelCase) },
    };

    public static string? Serialize(ToolAnnotations? annotations) =>
        annotations is null ? null : JsonSerializer.Serialize(annotations, Options);

    public static string? Serialize(ContentAnnotations? annotations) =>
        annotations is null ? null : JsonSerializer.Serialize(annotations, Options);
}
