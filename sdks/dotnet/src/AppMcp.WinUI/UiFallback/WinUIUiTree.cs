using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Automation.Peers;
using Microsoft.UI.Xaml.Automation.Provider;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Controls.Primitives;
using Microsoft.UI.Xaml.Media;
using AppMcp.UiFallback;

namespace AppMcp.WinUI.UiFallback;

// 从 WinUI 3 AutomationPeer 树收集兜底大纲条目（spec/ui-fallback.md 第 4–6 节、8.2 WinUI 列）。只能在 UI 线程上调用。

/// <summary>WinUI 控件节点的便捷访问。</summary>
internal static class WinUIUiNodeExtensions
{
    public static UIElement? Owner(this AutomationPeer peer) => (peer as FrameworkElementAutomationPeer)?.Owner;
    public static UIElement? Owner(this UiNode<AutomationPeer> n) => n.Node.Owner();
    public static T? Pattern<T>(this UiNode<AutomationPeer> n, PatternInterface p) where T : class => n.Node.GetPattern(p) as T;
}

/// <summary>一个顶层界面：窗口内容，或其上打开的弹出层（ContentDialog、浮出控件）。</summary>
internal sealed record WinUIUiRoot(AutomationPeer Peer, string Role, string Name, XamlRoot XamlRoot, Popup? Popup);

internal static class WinUIUiTree
{
    private readonly record struct Info(UiEntryKind Kind, string Role, int? Level = null);

    /// <summary>不再遍历子节点的控件（内部是文本宿主与滚动条，不是独立可操作控件）。</summary>
    private static readonly HashSet<string> LeafRoles = ["textbox", "slider", "spinbutton"];

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
        // 开关（ToggleSwitch）的控件类型是 Button 但带 Toggle 模式。
        if (p.Owner() is ToggleSwitch) return new(UiEntryKind.Item, "switch");
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

    /// <summary>密码类控件（PasswordBox）：值只给掩码，拒绝填写与按键。</summary>
    public static bool IsSecure(AutomationPeer p) => p.IsPassword() || p.Owner() is PasswordBox;

    /// <summary>密码类控件是否非空。</summary>
    /// <remarks>@security 只判断是否为空，值本身与长度都不输出；WinUI 没有 SecurePassword，读取后不保留引用。</remarks>
    public static bool SecureHasValue(AutomationPeer p) => p.Owner() is PasswordBox box && box.Password.Length > 0;

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

    /// <summary>选项的显示文本：ComboBoxItem 的内容，否则对象的字符串形式。</summary>
    public static string OptionText(object? item) => item switch
    {
        ContentControl { Content: { } content } => content.ToString() ?? "",
        null => "",
        _ => item.ToString() ?? "",
    };

    private static string? ComboValue(AutomationPeer p)
    {
        if (p.GetPattern(PatternInterface.Value) is IValueProvider v && !string.IsNullOrEmpty(v.Value)) return v.Value;
        return p.Owner() is ComboBox { SelectedItem: { } item } ? OptionText(item) : null;
    }

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
        if (p.HasKeyboardFocus()) states.Add("focused");
        return states;
    }

    /// <summary>
    /// 窗口当前可操作的顶层界面（按 Z 序，最上层在后）：窗口内容与打开的弹出层；
    /// ContentDialog 与轻触即关的弹出层遮挡窗口内容（spec/ui-fallback.md 4.4 被模态层遮挡）。
    /// </summary>
    public static List<WinUIUiRoot> Roots(Window window)
    {
        if (window.Content is not UIElement content || content.XamlRoot is not { } xamlRoot) return [];
        var roots = new List<WinUIUiRoot>();
        if (PeerOf(content) is { } main) roots.Add(new WinUIUiRoot(main, "window", window.Title ?? "", xamlRoot, null));
        foreach (var popup in VisualTreeHelper.GetOpenPopupsForXamlRoot(xamlRoot))
        {
            if (popup.Child is not UIElement child || PeerOf(child) is not { } peer) continue;
            var covering = child is ContentDialog || popup.IsLightDismissEnabled;
            if (covering) roots.Clear();
            var name = child is ContentDialog { Title: { } title } ? title.ToString() ?? "" : peer.GetName();
            roots.Add(new WinUIUiRoot(peer, "dialog", name, xamlRoot, popup));
        }
        return roots;
    }

    public static AutomationPeer? PeerOf(UIElement element) =>
        FrameworkElementAutomationPeer.FromElement(element) ?? FrameworkElementAutomationPeer.CreatePeerForElement(element);

    /// <summary>引用的控件仍在界面上（已加载、挂在某个 XamlRoot 上），但可能不可见。</summary>
    public static bool IsAlive(AutomationPeer peer) => peer.Owner() switch
    {
        FrameworkElement fe => fe.IsLoaded && fe.XamlRoot is not null,
        _ => true,
    };

    /// <summary>收集 <paramref name="roots"/>（或 <paramref name="start"/> 子树）下可见的条目，并为控件与分组分配引用。</summary>
    public static UiSnapshot<AutomationPeer> Collect(
        UiRefRegistry<AutomationPeer> refs, Func<UIElement, string?> declaredOf, IEnumerable<WinUIUiRoot> roots, AutomationPeer? start = null)
    {
        var tree = new UiTreeBuilder<AutomationPeer>();

        void Walk(AutomationPeer p)
        {
            if (p.IsOffscreen()) return;
            var info = Classify(p);
            if (info is { Kind: UiEntryKind.Heading } h)
            {
                tree.Heading(p, h.Level ?? 2, NameOf(p, h.Role));
            }
            else if (info is { Kind: UiEntryKind.Item } item)
            {
                tree.Item(p, item.Role, NameOf(p, item.Role), IsSecure(p));
                if (LeafRoles.Contains(item.Role)) return;
            }
            else if (info is { Kind: UiEntryKind.Container } container)
            {
                tree.BeginContainer(p, container.Role, NameOf(p, container.Role));
                foreach (var c in p.GetChildren() ?? []) Walk(c);
                tree.EndContainer();
                return;
            }
            foreach (var c in p.GetChildren() ?? []) Walk(c);
        }

        if (start is not null)
        {
            Walk(start);
        }
        else
        {
            foreach (var root in roots)
            {
                tree.BeginContainer(root.Peer, root.Role, UiOutlineFormat.Truncate(UiOutlineFormat.Collapse(root.Name), UiOutlineFormat.NameMax));
                foreach (var c in root.Peer.GetChildren() ?? []) Walk(c);
                tree.EndContainer();
            }
        }

        return tree.Build(refs, (peer, role, secure, kind) =>
        {
            var declared = peer.Owner() is { } owner ? declaredOf(owner) : null;
            return kind == UiEntryKind.Item
                ? new UiEntryDetails(ValueOf(peer, role, secure), StatesOf(peer, role), peer.IsRequiredForForm(), declared)
                : new UiEntryDetails(null, [], false, declared);
        });
    }
}
