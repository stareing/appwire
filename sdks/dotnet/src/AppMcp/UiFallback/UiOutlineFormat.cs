using System.Text;
using System.Text.Json.Serialization;
using System.Text.RegularExpressions;

namespace AppMcp.UiFallback;

// 进程内控件兜底的大纲与变化摘要格式：唯一定义见 spec/ui-fallback.md 第 4–7 节，本文件只是 C# 实现
// （与 UI 框架无关，WPF / WinUI 共用）。

/// <summary>条目类别：控件、分组（带引用，可用于 within）、标题（只作分组）。</summary>
internal enum UiEntryKind { Item, Container, Heading }

/// <summary>大纲中的一个条目。</summary>
internal sealed class UiEntry
{
    public required UiEntryKind Kind { get; init; }
    /// <summary>前后两次快照中对应同一控件的键（控件与分组为引用）。</summary>
    public required string Key { get; init; }
    public required string Role { get; init; }
    public required string Label { get; init; }
    public string? Ref { get; init; }
    public string Name { get; init; } = "";
    public string? Value { get; init; }
    public IReadOnlyList<string> States { get; init; } = [];
    public bool Required { get; init; }
    public string? Declared { get; init; }
    public int? Level { get; init; }
    /// <summary>所在分组层数（缩进）。</summary>
    public int Depth { get; init; }
    /// <summary>祖先分组（分组与标题）在条目列表中的下标，外层在前。</summary>
    public IReadOnlyList<int> Chain { get; init; } = [];
    /// <summary>祖先分组的键，外层在前。</summary>
    public IReadOnlyList<string> Containers { get; init; } = [];

    /// <summary><c>按钮「结算」</c>。</summary>
    public string Described => Name.Length == 0 ? Label : $"{Label}「{Name}」";
}

/// <summary><c>ui.outline</c> 的结构化条目（spec/ui-fallback.md 4.3）。</summary>
internal sealed record UiOutlineItem(
    [property: JsonPropertyName("ref")] string Ref,
    [property: JsonPropertyName("role")] string Role,
    [property: JsonPropertyName("name")] string Name)
{
    [JsonPropertyName("value"), JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)] public string? Value { get; init; }
    [JsonPropertyName("states"), JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)] public IReadOnlyList<string>? States { get; init; }
    [JsonPropertyName("required"), JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)] public bool? Required { get; init; }
    [JsonPropertyName("declared"), JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)] public string? Declared { get; init; }
    [JsonPropertyName("group"), JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)] public string? Group { get; init; }
}

/// <summary><c>ui.outline</c> 的结果（spec/ui-fallback.md 4.1）。</summary>
internal sealed record UiOutlineResult(
    [property: JsonPropertyName("text")] string Text,
    [property: JsonPropertyName("items")] IReadOnlyList<UiOutlineItem> Items,
    [property: JsonPropertyName("total")] int Total)
{
    [JsonPropertyName("remaining"), JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)] public int? Remaining { get; init; }
    [JsonPropertyName("hint"), JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)] public string? Hint { get; init; }
}

/// <summary>操作类工具的结果（spec/ui-fallback.md 7.2）。</summary>
internal sealed record UiActionResult([property: JsonPropertyName("changes")] IReadOnlyList<string> Changes)
{
    [JsonPropertyName("ok")] public bool Ok => true;
    [JsonPropertyName("hint"), JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)] public string? Hint { get; init; }
}

/// <summary><c>ui.read</c> 的结果。</summary>
internal sealed record UiReadResult(
    [property: JsonPropertyName("ref")] string Ref,
    [property: JsonPropertyName("text")] string Text,
    [property: JsonPropertyName("truncated")] bool Truncated);

internal static partial class UiOutlineFormat
{
    /// <summary>角色 → 中文标签（spec/ui-fallback.md 第 5 节）。</summary>
    public static readonly IReadOnlyDictionary<string, string> RoleLabels = new Dictionary<string, string>
    {
        ["button"] = "按钮", ["link"] = "链接", ["textbox"] = "输入框", ["searchbox"] = "搜索框",
        ["checkbox"] = "复选框", ["radio"] = "单选框", ["switch"] = "开关", ["combobox"] = "下拉框",
        ["listbox"] = "列表框", ["option"] = "选项", ["slider"] = "滑块", ["spinbutton"] = "数字框",
        ["tab"] = "标签页", ["menuitem"] = "菜单项", ["treeitem"] = "树节点", ["gridcell"] = "单元格",
        ["status"] = "提示", ["alert"] = "警告", ["generic"] = "元素",
        ["window"] = "窗口", ["dialog"] = "对话框", ["alertdialog"] = "警告框", ["navigation"] = "导航",
        ["main"] = "主区域", ["form"] = "表单", ["search"] = "搜索区", ["group"] = "分组", ["list"] = "列表",
        ["scrollable"] = "滚动区", ["heading"] = "标题",
    };

    /// <summary>密码类控件的标签与固定掩码（spec/ui-fallback.md 8.1：不泄露长度）。</summary>
    public const string SecureLabel = "密码框";
    public const string SecureMask = "••••";

    public const int NameMax = 40;
    public const int StatusNameMax = 80;
    public const int ValueMax = 30;
    public const int MaxChanges = 15;
    public const int LimitMax = 500;
    public const int ReadDefault = 1000;
    public const int ReadMax = 20000;

    /// <summary>引用格式（spec/ui-fallback.md 第 3 节）。</summary>
    public static readonly Regex RefPattern = RefRegex();

    [GeneratedRegex(@"^e[1-9]\d*$")]
    private static partial Regex RefRegex();

    [GeneratedRegex(@"\s+")]
    private static partial Regex Whitespace();

    public static string Label(string role) => RoleLabels.TryGetValue(role, out var l) ? l : "元素";

    public static string Collapse(string? s) => s is null ? "" : Whitespace().Replace(s, " ").Trim();

    /// <summary>截断到 max 字（保留前 max − 1 字、去掉尾部空白再加 <c>…</c>）。</summary>
    public static string Truncate(string s, int max)
    {
        var info = new System.Globalization.StringInfo(s);
        if (info.LengthInTextElements <= max) return s;
        return info.SubstringByTextElements(0, Math.Max(0, max - 1)).TrimEnd() + "…";
    }

    /// <summary><c>read</c> 的文本：折叠空白、相邻重复只保留一次（按钮名称与其内部文本相同）、截断到 max 字。</summary>
    public static UiReadResult ReadText(IEnumerable<string?> parts, string? reference, int max)
    {
        var texts = parts.Select(Collapse).Where(s => s.Length > 0).ToList();
        var full = string.Join(' ', texts.Where((s, i) => i == 0 || s != texts[i - 1]));
        var truncated = new System.Globalization.StringInfo(full).LengthInTextElements > max;
        return new UiReadResult(reference ?? "root", truncated ? Truncate(full, max) : full, truncated);
    }

    private static string GroupLabel(UiEntry e) => e.Kind == UiEntryKind.Heading ? e.Name : e.Described;

    private static string Haystack(UiEntry e, IReadOnlyList<UiEntry> entries)
    {
        var parts = new List<string> { e.Label, e.Role, e.Name, e.Value ?? "", e.Declared ?? "" };
        parts.AddRange(e.Chain.Select(i => GroupLabel(entries[i])));
        return string.Join(' ', parts).ToLowerInvariant();
    }

    private static string ItemLine(UiEntry e)
    {
        var b = new StringBuilder($"{e.Ref} {e.Described}");
        if (e.Value is not null) b.Append($"= \"{e.Value}\"");
        if (e.States.Count > 0) b.Append(' ').Append(string.Join(' ', e.States));
        if (e.Required) b.Append(" (必填)");
        if (e.Declared is not null) b.Append($" [已声明：{e.Declared}]");
        return b.ToString();
    }

    private static string ContainerLine(UiEntry e) =>
        $"» {e.Ref} {e.Described}" + (e.Declared is not null ? $" [已声明：{e.Declared}]" : "");

    private static string HeadingLine(UiEntry e) => $"{new string('#', e.Level ?? 2)} {e.Name}";

    /// <summary>过滤、截断、分组并渲染大纲（spec/ui-fallback.md 4.2 / 4.3）。</summary>
    public static UiOutlineResult Render(IReadOnlyList<UiEntry> entries, string? query, int limit)
    {
        var tokens = (query ?? "").ToLowerInvariant().Split((char[]?)null, StringSplitOptions.RemoveEmptyEntries);
        var matched = new List<int>();
        for (var i = 0; i < entries.Count; i++)
        {
            var e = entries[i];
            if (e.Kind != UiEntryKind.Item) continue;
            if (tokens.Length > 0)
            {
                var h = Haystack(e, entries);
                if (!tokens.All(h.Contains)) continue;
            }
            matched.Add(i);
        }
        var shown = matched.Take(limit).ToHashSet();
        var keptGroups = shown.SelectMany(i => entries[i].Chain).ToHashSet();

        var lines = new List<string>();
        var items = new List<UiOutlineItem>();
        var declared = false;
        for (var i = 0; i < entries.Count; i++)
        {
            var e = entries[i];
            var indent = new string(' ', e.Depth * 2);
            if (e.Kind == UiEntryKind.Item)
            {
                if (!shown.Contains(i)) continue;
                lines.Add(indent + ItemLine(e));
                var group = string.Join(" › ", e.Chain.Select(g => GroupLabel(entries[g])));
                declared |= e.Declared is not null;
                items.Add(new UiOutlineItem(e.Ref!, e.Role, e.Name)
                {
                    Value = e.Value,
                    States = e.States.Count > 0 ? e.States : null,
                    Required = e.Required ? true : null,
                    Declared = e.Declared,
                    Group = group.Length > 0 ? group : null,
                });
            }
            else if (keptGroups.Contains(i))
            {
                if (e.Kind == UiEntryKind.Container)
                {
                    lines.Add(indent + ContainerLine(e));
                    declared |= e.Declared is not null;
                }
                else
                {
                    lines.Add(indent + HeadingLine(e));
                }
            }
        }
        var remaining = matched.Count - shown.Count;
        if (lines.Count == 0) lines.Add(tokens.Length > 0 ? $"（没有与「{query}」匹配的可交互元素）" : "（没有可见的可交互元素）");
        if (remaining > 0) lines.Add($"…另有 {remaining} 个元素未列出，可用 query 或 within 缩小范围");
        return new UiOutlineResult(string.Join('\n', lines), items, matched.Count)
        {
            Remaining = remaining > 0 ? remaining : null,
            Hint = declared ? "标注 [已声明：…] 的元素已有对应工具，请优先直接调用该工具" : null,
        };
    }

    private static readonly HashSet<string> IgnoredStates = ["focused"];
    private static readonly string[][] ExclusiveStates = [["checked", "unchecked", "mixed"], ["expanded", "collapsed"]];

    private static IEnumerable<string> StateChange(IReadOnlyList<string> before, IReadOnlyList<string> after)
    {
        var b = before.Where(s => !IgnoredStates.Contains(s)).ToList();
        var a = after.Where(s => !IgnoredStates.Contains(s)).ToList();
        var added = a.Where(s => !b.Contains(s)).ToList();
        var removed = b.Where(s => !a.Contains(s))
            .Where(r => !ExclusiveStates.Any(g => g.Contains(r) && added.Any(g.Contains)))
            .ToList();
        if (added.Count > 0) yield return $"变为 {string.Join('、', added)}";
        if (removed.Count > 0) yield return $"不再 {string.Join('、', removed)}";
    }

    private static string VisibleStates(UiEntry e)
    {
        var s = e.States.Where(x => !IgnoredStates.Contains(x)).ToList();
        return s.Count == 0 ? "" : " " + string.Join(' ', s);
    }

    private static string ChangeLabel(UiEntry e) => e.Kind == UiEntryKind.Heading ? $"标题「{e.Name}」" : $"{e.Described}({e.Ref})";

    /// <summary>外层最先出现的"同样是新增 / 同样被移除"的祖先分组。</summary>
    private static string? OuterChanged(UiEntry e, Dictionary<string, UiEntry> set, Dictionary<string, UiEntry> other) =>
        e.Containers.FirstOrDefault(c => set.ContainsKey(c) && !other.ContainsKey(c));

    private static Dictionary<string, int> ChildCounts(
        IReadOnlyList<UiEntry> list, Dictionary<string, UiEntry> set, Dictionary<string, UiEntry> other)
    {
        var counts = new Dictionary<string, int>();
        foreach (var e in list)
        {
            if (other.ContainsKey(e.Key)) continue;
            var outer = OuterChanged(e, set, other);
            if (outer is not null) counts[outer] = counts.GetValueOrDefault(outer) + (e.Kind == UiEntryKind.Item ? 1 : 0);
        }
        return counts;
    }

    /// <summary>操作前后的变化摘要（spec/ui-fallback.md 7.2）。</summary>
    public static List<string> Diff(IReadOnlyList<UiEntry> before, IReadOnlyList<UiEntry> after, string prefix)
    {
        var byBefore = before.ToDictionary(e => e.Key);
        var byAfter = after.ToDictionary(e => e.Key);
        var added = ChildCounts(after, byAfter, byBefore);
        var removed = ChildCounts(before, byBefore, byAfter);
        var output = new List<string>();

        foreach (var e in after)
        {
            if (!byBefore.TryGetValue(e.Key, out var old))
            {
                if (OuterChanged(e, byAfter, byBefore) is not null) continue;
                var b = new StringBuilder($"新增{ChangeLabel(e)}");
                if (e.Kind == UiEntryKind.Item)
                {
                    if (e.Value is not null) b.Append($" = \"{e.Value}\"");
                    b.Append(VisibleStates(e));
                }
                if (added.TryGetValue(e.Key, out var n) && n > 0)
                {
                    b.Append($"，含 {n} 个可交互元素（可用 {prefix}.outline({{ within: \"{e.Ref}\" }}) 查看）");
                }
                output.Add(b.ToString());
                continue;
            }
            if (e.Kind == UiEntryKind.Heading)
            {
                if (old.Name != e.Name) output.Add($"标题「{old.Name}」变为「{e.Name}」");
                continue;
            }
            var parts = new List<string>();
            if (old.Name != e.Name) parts.Add($"名称变为「{e.Name}」");
            if (old.Value != e.Value) parts.Add(e.Value is null ? "值已清空" : $"值变为 \"{e.Value}\"");
            parts.AddRange(StateChange(old.States, e.States));
            if (parts.Count > 0) output.Add($"{e.Ref} {(old.Name.Length == 0 ? old.Label : old.Name)} {string.Join('，', parts)}");
        }

        foreach (var e in before)
        {
            if (byAfter.ContainsKey(e.Key) || OuterChanged(e, byBefore, byAfter) is not null) continue;
            output.Add($"{ChangeLabel(e)} 已消失" + (removed.TryGetValue(e.Key, out var n) && n > 0 ? $"（含 {n} 个可交互元素）" : ""));
        }

        if (output.Count > MaxChanges)
        {
            var rest = output.Count - MaxChanges;
            output.RemoveRange(MaxChanges, rest);
            output.Add($"…另有 {rest} 项变化，请调用 {prefix}.outline 查看");
        }
        return output;
    }
}
