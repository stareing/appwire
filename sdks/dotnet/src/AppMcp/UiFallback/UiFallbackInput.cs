using System.Text.Json;

namespace AppMcp.UiFallback;

/// <summary>兜底工具的参数解析与错误（spec/ui-fallback.md 第 2 节、7.1），各 UI 框架绑定共用。</summary>
internal static class UiFallbackInput
{
    public static ToolCallException Invalid(string message, string? reference = null, string? reason = null)
    {
        var details = new Dictionary<string, object?>();
        if (reference is not null) details["ref"] = reference;
        if (reason is not null) details["reason"] = reason;
        return details.Count == 0
            ? new ToolCallException(ToolErrorKind.InvalidInput, message)
            : new ToolCallException(ToolErrorKind.InvalidInput, message, (object)details);
    }

    public static ToolCallException Stale(string reference, string prefix) =>
        Invalid($"引用 {reference} 已失效，请重新调用 {prefix}.outline", reference);

    public static ToolCallException Hidden(string label, string reference, string prefix) =>
        Invalid($"{label}当前不可见，请重新调用 {prefix}.outline", reference, "hidden");

    public static ToolCallException Disabled(string label, string reference) =>
        Invalid($"{label}已禁用，当前无法操作", reference, "TOOL_DISABLED");

    public static ToolCallException Secure(string label, string? reference) =>
        Invalid($"{label}是密码类控件，兜底工具不填写", reference, "secure");

    public static ToolCallException Unsupported(string label, string reference, string? why = null) =>
        Invalid($"{label}不支持该操作" + (why is null ? "" : "：" + why), reference, "unsupported");

    private static JsonElement? Get(JsonElement args, string key) =>
        args.ValueKind == JsonValueKind.Object && args.TryGetProperty(key, out var v) && v.ValueKind != JsonValueKind.Null ? v : null;

    public static string? String(JsonElement args, string key)
    {
        if (Get(args, key) is not { } v) return null;
        if (v.ValueKind != JsonValueKind.String) throw Invalid($"{key} 应为字符串");
        return v.GetString();
    }

    public static int? Int(JsonElement args, string key)
    {
        if (Get(args, key) is not { } v) return null;
        if (v.ValueKind != JsonValueKind.Number || !v.TryGetDouble(out var d) || !double.IsFinite(d)) throw Invalid($"{key} 应为正整数");
        return (int)Math.Clamp(Math.Floor(d), int.MinValue, int.MaxValue);
    }

    public static string? Ref(JsonElement args, string key, bool required = true)
    {
        var v = Get(args, key);
        if (v is null || v.Value.ValueKind == JsonValueKind.String && v.Value.GetString() == "")
        {
            if (required) throw Invalid($"缺少参数 {key}（控件引用，如 \"e12\"）");
            return null;
        }
        var s = v.Value.ValueKind == JsonValueKind.String ? v.Value.GetString()!.Trim() : null;
        if (s is null || !UiOutlineFormat.RefPattern.IsMatch(s)) throw Invalid($"{key} 应为控件引用，如 \"e12\"");
        return s;
    }

    /// <summary><c>fill</c> 的 value：字符串、数字或布尔。</summary>
    public static object Value(JsonElement args)
    {
        if (args.ValueKind != JsonValueKind.Object || !args.TryGetProperty("value", out var v)) throw Invalid("缺少参数 value");
        return v.ValueKind switch
        {
            JsonValueKind.String => v.GetString()!,
            JsonValueKind.Number => v.GetDouble(),
            JsonValueKind.True => true,
            JsonValueKind.False => false,
            _ => throw Invalid("value 应为文本、数字或布尔"),
        };
    }

    /// <summary>输入 schema（spec/ui-fallback.md 第 2 节）。</summary>
    public static class Schemas
    {
        private const string RefProp = "{\"type\":\"string\",\"description\":\"控件引用，如 \\\"e12\\\"（来自 outline）\"}";

        public static string Outline(int maxItems) =>
            "{\"type\":\"object\",\"properties\":{" +
            "\"query\":{\"type\":\"string\",\"description\":\"按名称模糊过滤（空格分隔多个词，全部匹配）\"}," +
            "\"within\":{\"type\":\"string\",\"description\":\"只列出该引用分组的子树，如 \\\"e40\\\"\"}," +
            $"\"limit\":{{\"type\":\"integer\",\"minimum\":1,\"maximum\":{UiOutlineFormat.LimitMax},\"description\":\"最多列出的控件数，默认 {maxItems}\"}}" +
            "},\"additionalProperties\":false}";

        public const string Ref = "{\"type\":\"object\",\"properties\":{\"ref\":" + RefProp + "},\"required\":[\"ref\"],\"additionalProperties\":false}";

        public const string Fill =
            "{\"type\":\"object\",\"properties\":{\"ref\":" + RefProp + "," +
            "\"value\":{\"description\":\"文本、数字、布尔（复选框 / 开关 / 单选框）或选项文本（下拉框）\",\"anyOf\":[{\"type\":\"string\"},{\"type\":\"number\"},{\"type\":\"boolean\"}]}}," +
            "\"required\":[\"ref\",\"value\"],\"additionalProperties\":false}";

        public const string Press =
            "{\"type\":\"object\",\"properties\":{\"ref\":{\"type\":\"string\",\"description\":\"目标控件引用；缺省为当前焦点控件\"}," +
            "\"key\":{\"type\":\"string\",\"description\":\"按键，如 \\\"Enter\\\"、\\\"Escape\\\"、\\\"Tab\\\"、\\\"Shift+Tab\\\"、\\\"Space\\\"\"}}," +
            "\"required\":[\"key\"],\"additionalProperties\":false}";

        public const string Scroll =
            "{\"type\":\"object\",\"properties\":{\"ref\":" + RefProp + "," +
            "\"direction\":{\"type\":\"string\",\"enum\":[\"up\",\"down\",\"left\",\"right\"],\"description\":\"查看方向；缺省为滚动到该控件可见\"}}," +
            "\"required\":[\"ref\"],\"additionalProperties\":false}";

        public static readonly string Read =
            "{\"type\":\"object\",\"properties\":{\"ref\":{\"type\":\"string\",\"description\":\"控件引用；缺省为全部窗口\"}," +
            $"\"maxChars\":{{\"type\":\"integer\",\"minimum\":1,\"maximum\":{UiOutlineFormat.ReadMax},\"description\":\"默认 {UiOutlineFormat.ReadDefault}\"}}" +
            "},\"additionalProperties\":false}";
    }
}
