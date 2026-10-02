using System.Runtime.CompilerServices;

namespace AppMcp.UiFallback;

// 控件兜底中与 UI 框架无关的树收集部分（spec/ui-fallback.md 第 3–4 节）：引用登记、分组 / 标题路径与条目组装，
// WPF / WinUI 只负责遍历自己的控件树并给出每个节点的分类与细节。

/// <summary>一个可见控件（或分组）及其条目。</summary>
internal sealed record UiNode<TNode>(TNode Node, UiEntry Entry, bool Secure) where TNode : class
{
    public string Label => $"{Entry.Ref} {Entry.Described}";
}

/// <summary>一次收集的结果：条目（按树序）与引用 → 控件。</summary>
internal sealed record UiSnapshot<TNode>(IReadOnlyList<UiEntry> Entries, IReadOnlyDictionary<string, UiNode<TNode>> ByRef) where TNode : class;

/// <summary>节点的细节（值、状态等）；分组只用 <see cref="Declared"/>。</summary>
internal readonly record struct UiEntryDetails(string? Value, IReadOnlyList<string> States, bool Required, string? Declared);

/// <summary>引用 <c>eN</c> ↔ 控件对象：弱引用（spec/ui-fallback.md 第 3 节）。只在 UI 线程上访问。</summary>
internal sealed class UiRefRegistry<T> where T : class
{
    private ConditionalWeakTable<T, string> _byNode = new();
    private readonly Dictionary<string, WeakReference<T>> _byRef = new();
    private int _next = 1;

    public string RefOf(T node)
    {
        if (_byNode.TryGetValue(node, out var existing)) return existing;
        var r = $"e{_next++}";
        _byNode.Add(node, r);
        _byRef[r] = new WeakReference<T>(node);
        return r;
    }

    /// <summary>引用记录的控件对象（未被回收；可能已不在界面上）。</summary>
    public T? Target(string reference) => _byRef.TryGetValue(reference, out var weak) && weak.TryGetTarget(out var node) ? node : null;

    public void Prune()
    {
        foreach (var dead in _byRef.Where(kv => !kv.Value.TryGetTarget(out _)).Select(kv => kv.Key).ToList()) _byRef.Remove(dead);
    }

    public void Clear()
    {
        _byNode = new ConditionalWeakTable<T, string>();
        _byRef.Clear();
    }
}

/// <summary>
/// 按遍历顺序记录控件、分组与标题，维护分组 / 标题路径（缩进、group、within），最后分配引用并组装条目。
/// 遍历方先调 <see cref="BeginContainer"/>，遍历其子节点后调 <see cref="EndContainer"/>。
/// </summary>
internal sealed class UiTreeBuilder<TNode> where TNode : class
{
    private sealed class Frame(int container)
    {
        public int Container { get; } = container;
        public List<(int Level, int Index)> Headings { get; } = [];
    }

    private sealed record Raw(TNode Node, UiEntryKind Kind, string Role, string Name, int? Level, int Depth, List<int> Chain, List<int> Containers, bool Secure);

    private readonly List<Raw> _raw = [];
    private readonly List<Frame> _frames = [new(-1)];
    private readonly List<int> _containers = [];

    private List<int> Chain()
    {
        var c = new List<int>();
        foreach (var f in _frames)
        {
            if (f.Container >= 0) c.Add(f.Container);
            c.AddRange(f.Headings.Select(h => h.Index));
        }
        return c;
    }

    private void Add(TNode node, UiEntryKind kind, string role, string name, int? level, bool secure) =>
        _raw.Add(new Raw(node, kind, role, name, level, _frames.Count - 1, Chain(), [.. _containers], secure));

    public void Item(TNode node, string role, string name, bool secure) => Add(node, UiEntryKind.Item, role, name, null, secure);

    /// <summary>标题（级别 1–3）；同一分组中级别不低于它的前一个标题到此结束。</summary>
    public void Heading(TNode node, int level, string name)
    {
        if (name.Length == 0) return;
        level = Math.Clamp(level, 1, 3);
        var frame = _frames[^1];
        while (frame.Headings.Count > 0 && frame.Headings[^1].Level >= level) frame.Headings.RemoveAt(frame.Headings.Count - 1);
        Add(node, UiEntryKind.Heading, "heading", name, level, false);
        frame.Headings.Add((level, _raw.Count - 1));
    }

    public void BeginContainer(TNode node, string role, string name)
    {
        Add(node, UiEntryKind.Container, role, name, null, false);
        _frames.Add(new Frame(_raw.Count - 1));
        _containers.Add(_raw.Count - 1);
    }

    public void EndContainer()
    {
        _frames.RemoveAt(_frames.Count - 1);
        _containers.RemoveAt(_containers.Count - 1);
    }

    /// <summary>为控件与分组分配引用并组装条目。<paramref name="details"/> 对控件与分组调用（分组只取 Declared）。</summary>
    public UiSnapshot<TNode> Build(UiRefRegistry<TNode> refs, Func<TNode, string, bool, UiEntryKind, UiEntryDetails> details)
    {
        var refsByIndex = new string?[_raw.Count];
        for (var i = 0; i < _raw.Count; i++)
        {
            if (_raw[i].Kind != UiEntryKind.Heading) refsByIndex[i] = refs.RefOf(_raw[i].Node);
        }
        string KeyOf(int i) => refsByIndex[i] ?? $"h{RuntimeHelpers.GetHashCode(_raw[i].Node)}";

        var entries = new List<UiEntry>(_raw.Count);
        var byRef = new Dictionary<string, UiNode<TNode>>();
        for (var i = 0; i < _raw.Count; i++)
        {
            var r = _raw[i];
            var isItem = r.Kind == UiEntryKind.Item;
            var d = r.Kind == UiEntryKind.Heading ? default : details(r.Node, r.Role, r.Secure, r.Kind);
            var entry = new UiEntry
            {
                Kind = r.Kind,
                Key = KeyOf(i),
                Role = r.Role,
                Label = r.Secure ? UiOutlineFormat.SecureLabel : UiOutlineFormat.Label(r.Role),
                Ref = refsByIndex[i],
                Name = r.Name,
                Value = isItem ? d.Value : null,
                States = isItem ? d.States ?? [] : [],
                Required = isItem && d.Required,
                Declared = d.Declared,
                Level = r.Level,
                Depth = r.Depth,
                Chain = r.Chain,
                Containers = r.Containers.Select(KeyOf).ToList(),
            };
            entries.Add(entry);
            if (refsByIndex[i] is { } reference) byRef[reference] = new UiNode<TNode>(r.Node, entry, r.Secure);
        }
        refs.Prune();
        return new UiSnapshot<TNode>(entries, byRef);
    }
}

/// <summary>控件对象 → 绑定到它的工具名（大纲中的 <c>[已声明：…]</c>）。弱引用，控件回收即消失。</summary>
internal sealed class UiDeclaredTools
{
    private readonly ConditionalWeakTable<object, List<string>> _declared = new();

    public void Declare(object element, IEnumerable<string> names)
    {
        var list = _declared.GetOrCreateValue(element);
        lock (list) list.AddRange(names.Where(n => !list.Contains(n)).Distinct());
    }

    public void Undeclare(object element, IReadOnlyCollection<string> names)
    {
        if (!_declared.TryGetValue(element, out var list)) return;
        lock (list) list.RemoveAll(names.Contains);
    }

    /// <summary>绑定到该控件的工具名（多个以 ", " 连接）；没有时为 null。</summary>
    public string? Of(object element)
    {
        if (!_declared.TryGetValue(element, out var list)) return null;
        lock (list) return list.Count == 0 ? null : string.Join(", ", list);
    }
}
