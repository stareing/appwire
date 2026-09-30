using System.Text.Json;
using System.Text.Json.Nodes;
using System.Text.Json.Schema;
using System.Text.Json.Serialization.Metadata;

namespace AppMcp;

/// <summary>由 .NET 类型生成工具的 inputSchema（基于 <see cref="JsonSchemaExporter"/>）。</summary>
public static class ToolSchema
{
    /// <summary>生成 <typeparamref name="T"/> 的 JSON Schema 文本。根节点必须是对象。</summary>
    public static string For<T>(JsonSerializerOptions? options = null) => For(typeof(T), options);

    public static string For(Type type, JsonSerializerOptions? options = null)
    {
        options = Prepare(options);
        var node = JsonSchemaExporter.GetJsonSchemaAsNode(
            options,
            type,
            new JsonSchemaExporterOptions { TreatNullObliviousAsNonNullable = true });

        if (node is not JsonObject obj)
        {
            // true / {} 之类：任意值，按无约束对象处理。
            return """{"type":"object"}""";
        }

        // 根节点可能是 ["object","null"]，Host 要求 "object"。
        if (obj["type"] is JsonArray types && types.Any(t => t?.GetValue<string>() == "object"))
        {
            obj["type"] = "object";
        }
        if (obj["type"]?.GetValueKind() != JsonValueKind.String || obj["type"]!.GetValue<string>() != "object")
        {
            throw new ArgumentException($"工具参数类型 {type} 必须序列化为 JSON 对象", nameof(type));
        }
        return obj.ToJsonString();
    }

    /// <summary>确保选项带有类型解析器并已只读（Schema 导出与序列化都要求）。</summary>
    internal static JsonSerializerOptions Prepare(JsonSerializerOptions? options)
    {
        options ??= JsonSerializerOptions.Web;
        if (!options.IsReadOnly)
        {
            options.TypeInfoResolver ??= new DefaultJsonTypeInfoResolver();
            options.MakeReadOnly();
        }
        return options;
    }
}
