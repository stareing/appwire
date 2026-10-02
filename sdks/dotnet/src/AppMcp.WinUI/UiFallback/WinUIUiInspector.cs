using System.Runtime.InteropServices;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Automation.Peers;
using Microsoft.UI.Xaml.Automation.Provider;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using AppMcp.UiFallback;

namespace AppMcp.WinUI.UiFallback;

/// <summary>
/// 兜底工具的执行引擎（spec/ui-fallback.md 第 2–7 节，8.2 WinUI 列）：大纲、点击、填写、按键、滚动、读取。
/// 与客户端无关；所有方法都必须在 UI 线程上调用（<see cref="WinUIUiFallback"/> 负责切换）。
/// </summary>
internal sealed class WinUIUiInspector(
    string prefix, int maxItems, TimeSpan settleDelay, Func<IReadOnlyList<Window>> windows, Func<UIElement, string?> declaredOf)
    : UiInspectorCore<AutomationPeer>(prefix, maxItems)
{
    private List<WinUIUiRoot> Roots() => windows().Where(WinUIUiFallback.IsUsable).SelectMany(WinUIUiTree.Roots).ToList();

    protected override UiSnapshot<AutomationPeer> Collect(AutomationPeer? start) => WinUIUiTree.Collect(Refs, declaredOf, Roots(), start);

    protected override bool IsAlive(AutomationPeer node) => WinUIUiTree.IsAlive(node);

    protected override string NameOf(AutomationPeer node) => node.GetName();

    protected override bool IsEnabled(AutomationPeer node) => node.IsEnabled();

    /// <summary>让出调度器（低优先级：输入、布局与绑定先处理），再等 settleDelay 后再让出一次。</summary>
    protected override async Task Settle()
    {
        await Yield();
        if (settleDelay <= TimeSpan.Zero) return;
        await Task.Delay(settleDelay);
        await Yield();
    }

    private static Task Yield()
    {
        var queue = DispatcherQueue.GetForCurrentThread();
        if (queue is null) return Task.CompletedTask;
        var done = new TaskCompletionSource();
        if (!queue.TryEnqueue(DispatcherQueuePriority.Low, () => done.TrySetResult())) done.TrySetResult();
        return done.Task;
    }

    protected override Exception? MapActionError(Exception error, UiNode<AutomationPeer>? target) => error switch
    {
        ElementNotEnabledException => UiFallbackInput.Disabled(target?.Label ?? "控件", target?.Entry.Ref ?? ""),
        InvalidOperationException or COMException when target is not null => UiFallbackInput.Unsupported(target.Label, target.Entry.Ref!, error.Message),
        _ => null,
    };

    /// <summary>激活：Invoke → Toggle → SelectionItem.Select → ExpandCollapse。</summary>
    public override Task<UiActionResult> Click(string reference)
    {
        var before = Snapshot();
        var n = Actionable(reference, before);
        var run = Activation(n.Node) ?? throw UiFallbackInput.Unsupported(n.Label, reference);
        return Act(before, n, run);
    }

    private static Action? Activation(AutomationPeer p) =>
        p.GetPattern(PatternInterface.Invoke) is IInvokeProvider invoke ? invoke.Invoke
        : p.GetPattern(PatternInterface.Toggle) is IToggleProvider toggle ? toggle.Toggle
        : p.GetPattern(PatternInterface.SelectionItem) is ISelectionItemProvider item ? item.Select
        : p.GetPattern(PatternInterface.ExpandCollapse) is IExpandCollapseProvider ec ? () => ToggleExpand(ec)
        : null;

    private static void ToggleExpand(IExpandCollapseProvider ec)
    {
        if (ec.ExpandCollapseState == ExpandCollapseState.Collapsed) ec.Expand();
        else ec.Collapse();
    }

    /// <summary>填写：文本框 Value（再 UpdateSource）、复选框 / 开关 Toggle、单选框 Select、下拉框按选项文本、滑块 RangeValue。</summary>
    public override Task<UiActionResult> Fill(string reference, object value)
    {
        var before = Snapshot();
        var n = Actionable(reference, before);
        RefuseSecure(n);
        return n.Entry.Role switch
        {
            "textbox" => Act(before, n, () => SetText(n, value)),
            "checkbox" or "switch" => Act(before, n, () => SetToggle(n, value)),
            "radio" => Act(before, n, () => SetRadio(n, value)),
            "combobox" => Act(before, n, () => SelectOption(n, value)),
            "slider" or "spinbutton" => Act(before, n, () => SetRange(n, value)),
            _ => throw UiFallbackInput.Unsupported(n.Label, reference, "只能填写文本框、复选框、开关、单选框、下拉框与滑块"),
        };
    }

    private static void SetText(UiNode<AutomationPeer> n, object value)
    {
        var text = UiFillValue.Text(n.Label, n.Entry.Ref, value);
        if (n.Pattern<IValueProvider>(PatternInterface.Value) is not { } v) throw UiFallbackInput.Unsupported(n.Label, n.Entry.Ref!);
        if (v.IsReadOnly) throw UiFallbackInput.Unsupported(n.Label, n.Entry.Ref!, "只读");
        v.SetValue(text);
        // @why Value 模式只改 Text；{Binding} 的默认 LostFocus 不会写回数据源，需显式 UpdateSource（x:Bind 无绑定表达式，不受影响之外无法补写）。
        if (n.Owner() is TextBox box) box.GetBindingExpression(TextBox.TextProperty)?.UpdateSource();
    }

    private static void SetToggle(UiNode<AutomationPeer> n, object value)
    {
        var want = UiFillValue.Bool(n.Label, n.Entry.Ref, value);
        if (n.Pattern<IToggleProvider>(PatternInterface.Toggle) is not { } t) throw UiFallbackInput.Unsupported(n.Label, n.Entry.Ref!);
        // 三态复选框按 Off → On → Indeterminate 循环，最多切换两次。
        for (var i = 0; i < 3 && (t.ToggleState == ToggleState.On) != want; i++) t.Toggle();
    }

    private static void SetRadio(UiNode<AutomationPeer> n, object value)
    {
        var want = UiFillValue.Bool(n.Label, n.Entry.Ref, value);
        if (n.Pattern<ISelectionItemProvider>(PatternInterface.SelectionItem) is not { } s) throw UiFallbackInput.Unsupported(n.Label, n.Entry.Ref!);
        if (want) s.Select();
        else if (s.IsSelected) throw UiFallbackInput.Unsupported(n.Label, n.Entry.Ref!, "单选框不能直接取消选中，请选中同组的其他项");
    }

    /// <summary>下拉框：可编辑时直接写值；否则按选项文本（先精确后忽略大小写）设置 SelectedIndex。</summary>
    /// <remarks>@why WinUI 的下拉项在展开后才异步生成，按 Expand → 选项 Select 的方式在同一次调用内拿不到选项。</remarks>
    private static void SelectOption(UiNode<AutomationPeer> n, object value)
    {
        var text = UiFillValue.Text(n.Label, n.Entry.Ref, value);
        if (n.Owner() is ComboBox { IsEditable: true } && n.Pattern<IValueProvider>(PatternInterface.Value) is { IsReadOnly: false } editable)
        {
            editable.SetValue(text);
            return;
        }
        if (n.Owner() is not ComboBox combo) throw UiFallbackInput.Unsupported(n.Label, n.Entry.Ref!);
        var options = combo.Items.Select(WinUIUiTree.OptionText).ToList();
        var index = options.IndexOf(text);
        if (index < 0) index = options.FindIndex(o => string.Equals(o, text, StringComparison.OrdinalIgnoreCase));
        if (index < 0)
        {
            var names = string.Join("、", options.Where(s => s.Length > 0).Take(20));
            throw UiFallbackInput.Invalid($"{n.Label}没有选项「{text}」；可选：{names}", n.Entry.Ref);
        }
        combo.SelectedIndex = index;
    }

    private static void SetRange(UiNode<AutomationPeer> n, object value)
    {
        var number = UiFillValue.Number(n.Label, n.Entry.Ref, value);
        if (n.Pattern<IRangeValueProvider>(PatternInterface.RangeValue) is not { } r) throw UiFallbackInput.Unsupported(n.Label, n.Entry.Ref!);
        r.SetValue(number);
    }

    /// <summary>
    /// 按键：ref 给出时先聚焦该控件。WinUI 不能合成键盘输入，按语义执行：Tab 走 FocusManager；Enter / Space 激活焦点控件，
    /// 文本框中的 Enter 交给打开的 ContentDialog 的默认按钮；Escape 关闭最上层的对话框 / 浮出层。
    /// </summary>
    public override Task<UiActionResult> Press(string? reference, string key)
    {
        var kind = UiFallbackInput.Key(key);
        var before = Snapshot();
        var target = reference is null ? null : Actionable(reference, before);
        if (target is not null) RefuseSecure(target);
        (target?.Owner() as Control)?.Focus(FocusState.Programmatic);
        var roots = Roots();
        var xamlRoot = (target?.Owner() as FrameworkElement)?.XamlRoot ?? roots.LastOrDefault()?.XamlRoot;
        var focused = xamlRoot is null ? null : FocusManager.GetFocusedElement(xamlRoot) as UIElement;
        if (focused is PasswordBox) throw UiFallbackInput.Secure("当前焦点控件", reference);
        var handled = true;
        return ActKey(before, target, () => handled = ApplyKey(kind, focused, roots, xamlRoot), () => handled);
    }

    private async Task<UiActionResult> ActKey(UiSnapshot<AutomationPeer> before, UiNode<AutomationPeer>? target, Action run, Func<bool> handled)
    {
        var result = await Act(before, target, run);
        return handled() ? result : result with { Hint = result.Hint ?? "按键没有被任何控件处理" };
    }

    private static bool ApplyKey(UiKey kind, UIElement? focused, List<WinUIUiRoot> roots, XamlRoot? xamlRoot) => kind switch
    {
        UiKey.Tab or UiKey.ShiftTab => xamlRoot is not null && FocusManager.TryMoveFocus(
            kind == UiKey.Tab ? FocusNavigationDirection.Next : FocusNavigationDirection.Previous,
            new FindNextElementOptions { SearchRoot = xamlRoot.Content }),
        UiKey.Enter => (focused is TextBox && DialogButton(roots, defaultButton: true)) || Activate(focused),
        UiKey.Space => Activate(focused),
        UiKey.Escape => DialogButton(roots, defaultButton: false) || ClosePopup(roots),
        _ => false,
    };

    private static bool Activate(UIElement? element)
    {
        if (element is null || WinUIUiTree.PeerOf(element) is not { } peer || !peer.IsEnabled() || Activation(peer) is not { } run) return false;
        run();
        return true;
    }

    /// <summary>最上层 ContentDialog 的默认按钮（Enter）/ 关闭按钮（Escape）；模板部件名即其 AutomationId。</summary>
    private static bool DialogButton(List<WinUIUiRoot> roots, bool defaultButton)
    {
        if (roots.LastOrDefault()?.Popup?.Child is not ContentDialog dialog) return false;
        var part = defaultButton
            ? dialog.DefaultButton switch
            {
                ContentDialogButton.Primary => "PrimaryButton",
                ContentDialogButton.Secondary => "SecondaryButton",
                ContentDialogButton.Close => "CloseButton",
                _ => null,
            }
            : "CloseButton";
        if (part is not null && FindByName(dialog, part) is Button { Visibility: Visibility.Visible, IsEnabled: true } button)
        {
            return Activate(button);
        }
        if (defaultButton) return false;
        dialog.Hide();
        return true;
    }

    private static FrameworkElement? FindByName(DependencyObject root, string name)
    {
        for (var i = 0; i < VisualTreeHelper.GetChildrenCount(root); i++)
        {
            var child = VisualTreeHelper.GetChild(root, i);
            if (child is FrameworkElement fe && fe.Name == name) return fe;
            if (FindByName(child, name) is { } found) return found;
        }
        return null;
    }

    /// <summary>轻触即关的弹出层（浮出控件、菜单）：Escape 关闭。</summary>
    private static bool ClosePopup(List<WinUIUiRoot> roots)
    {
        if (roots.LastOrDefault()?.Popup is not { IsLightDismissEnabled: true } popup) return false;
        popup.IsOpen = false;
        return true;
    }

    private static (ScrollAmount Horizontal, ScrollAmount Vertical) AmountOf(UiScrollDirection d) => d switch
    {
        UiScrollDirection.Down => (ScrollAmount.NoAmount, ScrollAmount.LargeIncrement),
        UiScrollDirection.Up => (ScrollAmount.NoAmount, ScrollAmount.LargeDecrement),
        UiScrollDirection.Right => (ScrollAmount.LargeIncrement, ScrollAmount.NoAmount),
        _ => (ScrollAmount.LargeDecrement, ScrollAmount.NoAmount),
    };

    /// <summary>无方向：滚动到可见（ScrollItem 或 StartBringIntoView）；有方向：最近的可滚动祖先（含自身）滚动一页。</summary>
    public override Task<UiActionResult> Scroll(string reference, string? direction)
    {
        var dir = UiFallbackInput.Direction(direction);
        var before = Snapshot();
        var peer = before.ByRef.TryGetValue(reference, out var visible) ? visible.Node
            : Alive(reference) ?? throw UiFallbackInput.Stale(reference, Prefix);
        if (dir is not { } d)
        {
            if (visible is not null) return Unchanged();
            return Act(before, null, () => ScrollIntoView(peer));
        }
        var amount = AmountOf(d);
        // @why WinUI 3 的 AutomationPeer 不公开父节点：沿可视树向上取各元素的 peer。
        for (DependencyObject? cur = peer.Owner(); cur is not null; cur = VisualTreeHelper.GetParent(cur))
        {
            if (cur is UIElement e && WinUIUiTree.PeerOf(e)?.GetPattern(PatternInterface.Scroll) is IScrollProvider s && CanScroll(s, amount))
            {
                return Act(before, null, () => s.Scroll(amount.Horizontal, amount.Vertical));
            }
        }
        return Unchanged("已到尽头或不可滚动");
    }

    private static bool CanScroll(IScrollProvider s, (ScrollAmount Horizontal, ScrollAmount Vertical) a) =>
        a.Vertical switch
        {
            ScrollAmount.LargeIncrement => s.VerticallyScrollable && s.VerticalScrollPercent < 100,
            ScrollAmount.LargeDecrement => s.VerticallyScrollable && s.VerticalScrollPercent > 0,
            _ => a.Horizontal == ScrollAmount.LargeIncrement
                ? s.HorizontallyScrollable && s.HorizontalScrollPercent < 100
                : s.HorizontallyScrollable && s.HorizontalScrollPercent > 0,
        };

    private static void ScrollIntoView(AutomationPeer peer)
    {
        if (peer.GetPattern(PatternInterface.ScrollItem) is IScrollItemProvider item) item.ScrollIntoView();
        else peer.Owner()?.StartBringIntoView();
    }

    /// <summary>可见文本：名称与值（密码类只给掩码），折叠空白。</summary>
    public override UiReadResult Read(string? reference, int? maxChars)
    {
        var max = Math.Clamp(maxChars ?? UiOutlineFormat.ReadDefault, 1, UiOutlineFormat.ReadMax);
        var parts = new List<string>();
        void Walk(AutomationPeer p)
        {
            if (p.IsOffscreen()) return;
            parts.Add(p.GetName());
            if (WinUIUiTree.IsSecure(p))
            {
                if (WinUIUiTree.SecureHasValue(p)) parts.Add(UiOutlineFormat.SecureMask);
                return;
            }
            if (p.GetPattern(PatternInterface.Value) is IValueProvider v && v.Value is { Length: > 0 } value && value != p.GetName()) parts.Add(value);
            if (p.GetAutomationControlType() == AutomationControlType.Edit) return;
            foreach (var c in p.GetChildren() ?? []) Walk(c);
        }
        if (reference is not null)
        {
            Walk(Resolve(reference, Snapshot()).Node);
        }
        else
        {
            foreach (var root in Roots())
            {
                parts.Add(root.Name);
                Walk(root.Peer);
            }
        }
        return UiOutlineFormat.ReadText(parts, reference, max);
    }
}
