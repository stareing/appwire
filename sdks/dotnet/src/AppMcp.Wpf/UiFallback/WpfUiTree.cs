using System.Runtime.CompilerServices;
using System.Windows;
using System.Windows.Automation;
using System.Windows.Automation.Peers;
using System.Windows.Automation.Provider;
using System.Windows.Controls;
using System.Windows.Interop;
using AppMcp.UiFallback;

namespace AppMcp.Wpf.UiFallback;

// 从 WPF AutomationPeer 树收集兜底大纲条目（spec/ui-fallback.md 第 4–6 节、8.2 WPF 列）。只能在 UI 线程上调用。

/// <summary>一个可见控件（或分组）及其条目。</summary>
internal sealed record WpfUiNode(AutomationPeer Peer, UiEntry Entry, bool Secure)
{
    public UIElement? Owner => (Peer as UIElementAutomationPeer)?.Owner;
    public string Label => $"{Entry.Ref} {Entry.Described}";
    public T? Pattern<T>(PatternInterface p) where T : class => Peer.GetPattern(p) as T;
}

/// <summary>一次收集的结果：条目（按树序）与引用 → 控件。</summary>
internal sealed record WpfUiSnapshot(IReadOnlyList<UiEntry> Entries, IReadOnlyDictionary<string, WpfUiNode> ByRef);

/// <summary>引用 <c>eN</c> ↔ AutomationPeer：弱引用（spec/ui-fallback.md 第 3 节）。</summary>
internal sealed class WpfUiRefRegistry
{
    private ConditionalWeakTable<AutomationPeer, string> _byPeer = new();
    private readonly Dictionary<string, WeakReference<AutomationPeer>> _byRef = new();
    private int _next = 1;

    public string RefOf(AutomationPeer peer)
    {
        if (_byPeer.TryGetValue(peer, out var existing)) return existing;
        var r = $"e{_next++}";
        _byPeer.Add(peer, r);
        _byRef[r] = new WeakReference<AutomationPeer>(peer);
        return r;
    }

    /// <summary>引用对应的控件仍在（已加载、所在窗口未关闭），但可能不可见。</summary>
    public AutomationPeer? Alive(string reference)
    {
        if (!_byRef.TryGetValue(reference, out var weak) || !weak.TryGetTarget(out var peer)) return null;
        return peer switch
        {
            UIElementAutomationPeer { Owner: FrameworkElement fe } when !fe.IsLoaded || PresentationSource.FromVisual(fe) is null => null,
            _ => peer,
        };
    }

    public void Prune()
    {
        foreach (var dead in _byRef.Where(kv => !kv.Value.TryGetTarget(out _)).Select(kv => kv.Key).ToList()) _byRef.Remove(dead);
    }

    public void Clear()
    {
        _byPeer = new ConditionalWeakTable<AutomationPeer, string>();
        _byRef.Clear();
    }
}

internal static class WpfUiTree
{
    private readonly record struct Info(UiEntryKind Kind, string Role, int? Level = null);

    /// <summary>不再遍历子节点的控件（内部是文本宿主与滚动条，不是独立可操作控件）。</summary>
    private static readonly HashSet<string> LeafRoles = ["textbox", "slider", "spinbutton"];

    /// <summary>控件类型 → 角色（顺序无关；模式相关的判定见 <see cref="Classify"/>）。</summary>
    private static readonly Dictionary<AutomationControlType, string> ItemRoles = new()
    {
        [AutomationControlType.Button] = "button",
        [AutomationControlType.SplitButton] = "button",
        [AutomationControlType.Hyperlink] = "link",
        [AutomationControlType.Edit] = "textbox",
        [AutomationControlType.Document] = "textbox",
        [AutomationControlType.CheckBox] = "checkbox",
        [AutomationControlType.RadioButton] = "radio",
        [AutomationControlType.ComboBox] = "combobox",
        [AutomationControlType.ListItem] = "option",
        [AutomationControlType.DataItem] = "option",
        [AutomationControlType.Slider] = "slider",
        [AutomationControlType.Spinner] = "spinbutton",
        [AutomationControlType.TabItem] = "tab",
        [AutomationControlType.MenuItem] = "menuitem",
        [AutomationControlType.TreeItem] = "treeitem",
    };

    private static readonly Dictionary<AutomationControlType, string> ContainerRoles = new()
    {
        [AutomationControlType.List] = "list",
        [AutomationControlType.DataGrid] = "list",
        [AutomationControlType.Tree] = "list",
        [AutomationControlType.Menu] = "navigation",
        [AutomationControlType.MenuBar] = "navigation",
        [AutomationControlType.Group] = "group",
    };

    private static Info? Classify(AutomationPeer p)
    {
        var type = p.GetAutomationControlType();
        // 可展开的分组（Expander）作为按钮列出，展开 / 收起即点击。
        if (type == AutomationControlType.Group && p.GetPattern(PatternInterface.ExpandCollapse) is not null) return new(UiEntryKind.Item, "button");
        if (ItemRoles.TryGetValue(type, out var role)) return new(UiEntryKind.Item, role);
        if (ContainerRoles.TryGetValue(type, out var container)) return new(UiEntryKind.Container, container);
        var heading = p.GetHeadingLevel();
        if (heading != AutomationHeadingLevel.None && !string.IsNullOrWhiteSpace(p.GetName()))
        {
            return new(UiEntryKind.Heading, "heading", Math.Clamp((int)heading - (int)AutomationHeadingLevel.Level1 + 1, 1, 3));
        }
        if (p.GetLiveSetting() != AutomationLiveSetting.Off && !string.IsNullOrWhiteSpace(p.GetName())) return new(UiEntryKind.Item, "status");
        if (p.GetPattern(PatternInterface.Scroll) is IScrollProvider { VerticallyScrollable: true } or IScrollProvider { HorizontallyScrollable: true })
        {
            return new(UiEntryKind.Container, "scrollable");
        }
        if (p.GetPattern(PatternInterface.Invoke) is not null) return new(UiEntryKind.Item, "generic");
        return null;
    }

    private static string NameOf(AutomationPeer p, string role)
    {
        var name = UiOutlineFormat.Collapse(p.GetName());
        if (name.Length == 0) name = UiOutlineFormat.Collapse(p.GetHelpText());
        return UiOutlineFormat.Truncate(name, role is "status" or "alert" ? UiOutlineFormat.StatusNameMax : UiOutlineFormat.NameMax);
    }

    /// <summary>密码类控件（PasswordBox 等）：值只给掩码，拒绝填写与按键。</summary>
    public static bool IsSecure(AutomationPeer p) => p.IsPassword();

    private static string? ValueOf(AutomationPeer p, string role, bool secure)
    {
        if (secure) return SecureHasValue(p) ? UiOutlineFormat.SecureMask : null;
        string? raw = role switch
        {
            "slider" or "spinbutton" => p.GetPattern(PatternInterface.RangeValue) is IRangeValueProvider r ? r.Value.ToString(System.Globalization.CultureInfo.InvariantCulture) : null,
            "combobox" => ComboValue(p),
            _ => p.GetPattern(PatternInterface.Value) is IValueProvider v ? v.Value : null,
        };
        var collapsed = UiOutlineFormat.Collapse(raw);
        return collapsed.Length == 0 ? null : UiOutlineFormat.Truncate(collapsed, UiOutlineFormat.ValueMax);
    }

    /// <summary>密码类控件是否非空。</summary>
    /// <remarks>@security 只判断是否为空，值本身与长度都不输出；用 SecurePassword（不产生明文字符串副本）并立即释放。</remarks>
    public static bool SecureHasValue(AutomationPeer p)
    {
        if ((p as UIElementAutomationPeer)?.Owner is not PasswordBox box) return false;
        using var secret = box.SecurePassword;
        return secret.Length > 0;
    }

    private static string? ComboValue(AutomationPeer p)
    {
        if (p.GetPattern(PatternInterface.Value) is IValueProvider v && !string.IsNullOrEmpty(v.Value)) return v.Value;
        if (p.GetPattern(PatternInterface.Selection) is not ISelectionProvider s) return null;
        var names = s.GetSelection()?.Select(NameOfProvider).Where(n => n.Length > 0).ToList();
        return names is { Count: > 0 } ? string.Join('、', names) : null;
    }

    /// <summary>选中项的名称（IRawElementProviderSimple → 属性 Name）。</summary>
    private static string NameOfProvider(IRawElementProviderSimple provider) =>
        provider.GetPropertyValue(AutomationElementIdentifiers.NameProperty.Id) as string ?? "";

    private static List<string> StatesOf(AutomationPeer p, string role)
    {
        var states = new List<string>();
        if (!p.IsEnabled()) states.Add("disabled");
        if (p.GetPattern(PatternInterface.Toggle) is IToggleProvider t)
        {
            states.Add(t.ToggleState switch { ToggleState.On => "checked", ToggleState.Off => "unchecked", _ => "mixed" });
        }
        if (role == "radio" && p.GetPattern(PatternInterface.SelectionItem) is ISelectionItemProvider r)
        {
            states.Add(r.IsSelected ? "checked" : "unchecked");
        }
        else if (p.GetPattern(PatternInterface.SelectionItem) is ISelectionItemProvider { IsSelected: true })
        {
            states.Add("selected");
        }
        if (p.GetPattern(PatternInterface.ExpandCollapse) is IExpandCollapseProvider e)
        {
            switch (e.ExpandCollapseState)
            {
                case ExpandCollapseState.Expanded:
                case ExpandCollapseState.PartiallyExpanded:
                    states.Add("expanded");
                    break;
                case ExpandCollapseState.Collapsed:
                    states.Add("collapsed");
                    break;
            }
        }
        if (role == "textbox" && p.GetPattern(PatternInterface.Value) is IValueProvider { IsReadOnly: true }) states.Add("readonly");
        if ((p as UIElementAutomationPeer)?.Owner is DependencyObject d && System.Windows.Controls.Validation.GetHasError(d)) states.Add("invalid");
        if (p.HasKeyboardFocus()) states.Add("focused");
        return states;
    }

    /// <summary>窗口可被操作：可见、未最小化、未被模态对话框禁用。</summary>
    public static bool WindowUsable(Window w)
    {
        if (!w.IsVisible || w.WindowState == WindowState.Minimized) return false;
        var hwnd = new WindowInteropHelper(w).Handle;
        return hwnd == IntPtr.Zero || Win32.IsWindowEnabled(hwnd);
    }

    /// <summary>应用当前可见的窗口（UI 线程）。</summary>
    public static IEnumerable<Window> UsableWindows()
    {
        var app = Application.Current;
        if (app is null) yield break;
        foreach (Window w in app.Windows)
        {
            if (WindowUsable(w)) yield return w;
        }
    }

    private sealed class Frame(int container)
    {
        public int Container { get; } = container;
        public List<(int Level, int Index)> Headings { get; } = [];
    }

    private sealed record Raw(AutomationPeer Peer, Info Info, string Name, int Depth, List<int> Chain, List<int> Containers, bool Secure);

    /// <summary>收集 <paramref name="start"/>（缺省为全部可见窗口）下可见的条目，并为控件与分组分配引用。</summary>
    public static WpfUiSnapshot Collect(WpfUiRefRegistry refs, Func<UIElement, string?> declaredOf, AutomationPeer? start = null)
    {
        var raw = new List<Raw>();
        var frames = new List<Frame> { new(-1) };
        var containerStack = new List<int>();

        List<int> Chain()
        {
            var c = new List<int>();
            foreach (var f in frames)
            {
                if (f.Container >= 0) c.Add(f.Container);
                c.AddRange(f.Headings.Select(h => h.Index));
            }
            return c;
        }

        void Push(AutomationPeer peer, Info info, string name, bool secure)
        {
            raw.Add(new Raw(peer, info, name, frames.Count - 1, Chain(), [.. containerStack], secure));
            if (info.Kind != UiEntryKind.Container) return;
            frames.Add(new Frame(raw.Count - 1));
            containerStack.Add(raw.Count - 1);
        }

        void Pop()
        {
            frames.RemoveAt(frames.Count - 1);
            containerStack.RemoveAt(containerStack.Count - 1);
        }

        void Walk(AutomationPeer p)
        {
            if (p.IsOffscreen()) return;
            var info = Classify(p);
            var pushed = false;
            if (info is { } i)
            {
                if (i.Kind == UiEntryKind.Heading)
                {
                    var frame = frames[^1];
                    while (frame.Headings.Count > 0 && frame.Headings[^1].Level >= i.Level) frame.Headings.RemoveAt(frame.Headings.Count - 1);
                    raw.Add(new Raw(p, i, NameOf(p, i.Role), frames.Count - 1, Chain(), [.. containerStack], false));
                    frame.Headings.Add((i.Level ?? 2, raw.Count - 1));
                }
                else
                {
                    Push(p, i, NameOf(p, i.Role), IsSecure(p));
                    pushed = i.Kind == UiEntryKind.Container;
                }
                if (i.Kind == UiEntryKind.Item && LeafRoles.Contains(i.Role)) return;
            }
            foreach (var c in p.GetChildren() ?? []) Walk(c);
            if (pushed) Pop();
        }

        if (start is not null)
        {
            Walk(start);
        }
        else
        {
            foreach (var w in UsableWindows())
            {
                var peer = UIElementAutomationPeer.CreatePeerForElement(w);
                if (peer is null) continue;
                var role = w.Owner is not null || ComponentDispatcher.IsThreadModal ? "dialog" : "window";
                Push(peer, new Info(UiEntryKind.Container, role), UiOutlineFormat.Truncate(UiOutlineFormat.Collapse(w.Title), UiOutlineFormat.NameMax), false);
                foreach (var c in peer.GetChildren() ?? []) Walk(c);
                Pop();
            }
        }

        var refsByIndex = new string?[raw.Count];
        for (var i = 0; i < raw.Count; i++)
        {
            if (raw[i].Info.Kind != UiEntryKind.Heading) refsByIndex[i] = refs.RefOf(raw[i].Peer);
        }
        string KeyOf(int i) => refsByIndex[i] ?? $"h{RuntimeHelpers.GetHashCode(raw[i].Peer)}";

        var entries = new List<UiEntry>(raw.Count);
        var byRef = new Dictionary<string, WpfUiNode>();
        for (var i = 0; i < raw.Count; i++)
        {
            var r = raw[i];
            var isItem = r.Info.Kind == UiEntryKind.Item;
            var owner = (r.Peer as UIElementAutomationPeer)?.Owner;
            var entry = new UiEntry
            {
                Kind = r.Info.Kind,
                Key = KeyOf(i),
                Role = r.Info.Role,
                Label = r.Secure ? UiOutlineFormat.SecureLabel : UiOutlineFormat.Label(r.Info.Role),
                Ref = refsByIndex[i],
                Name = r.Name,
                Value = isItem ? ValueOf(r.Peer, r.Info.Role, r.Secure) : null,
                States = isItem ? StatesOf(r.Peer, r.Info.Role) : [],
                Required = isItem && r.Peer.IsRequiredForForm(),
                Declared = r.Info.Kind != UiEntryKind.Heading && owner is not null ? declaredOf(owner) : null,
                Level = r.Info.Level,
                Depth = r.Depth,
                Chain = r.Chain,
                Containers = r.Containers.Select(KeyOf).ToList(),
            };
            entries.Add(entry);
            if (refsByIndex[i] is { } reference) byRef[reference] = new WpfUiNode(r.Peer, entry, r.Secure);
        }
        refs.Prune();
        return new WpfUiSnapshot(entries, byRef);
    }
}

internal static partial class Win32
{
    /// <summary>窗口是否接受输入（模态对话框打开时，同线程的其他顶层窗口被禁用）。</summary>
    [System.Runtime.InteropServices.LibraryImport("user32.dll")]
    [return: System.Runtime.InteropServices.MarshalAs(System.Runtime.InteropServices.UnmanagedType.Bool)]
    public static partial bool IsWindowEnabled(IntPtr hwnd);
}
