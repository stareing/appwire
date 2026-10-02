using System.Globalization;
using System.Windows;
using System.Windows.Automation;
using System.Windows.Automation.Peers;
using System.Windows.Automation.Provider;
using System.Windows.Controls;
using System.Windows.Input;
using System.Windows.Threading;
using AppMcp.UiFallback;

namespace AppMcp.Wpf.UiFallback;

/// <summary>
/// 兜底工具的执行引擎（spec/ui-fallback.md 第 2–7 节，8.2 WPF 列）：大纲、点击、填写、按键、滚动、读取。
/// 与客户端无关；所有方法都必须在 UI 线程上调用（<see cref="WpfUiFallback"/> 负责切换）。
/// </summary>
internal sealed class WpfUiInspector(string prefix, int maxItems, TimeSpan settleDelay, Func<UIElement, string?> declaredOf)
    : UiInspectorCore<AutomationPeer>(prefix, maxItems)
{
    protected override UiSnapshot<AutomationPeer> Collect(AutomationPeer? start) => WpfUiTree.Collect(Refs, declaredOf, start);

    protected override bool IsAlive(AutomationPeer node) => WpfUiTree.IsAlive(node);

    protected override string NameOf(AutomationPeer node) => node.GetName();

    protected override bool IsEnabled(AutomationPeer node) => node.IsEnabled();

    private static Dispatcher Dispatcher => Application.Current?.Dispatcher ?? Dispatcher.CurrentDispatcher;

    /// <summary>让出调度器直到空闲（处理 Invoke 投递的点击、布局与绑定），再等 settleDelay 后再空闲一次。</summary>
    protected override async Task Settle()
    {
        await Dispatcher.InvokeAsync(static () => { }, DispatcherPriority.ApplicationIdle);
        if (settleDelay <= TimeSpan.Zero) return;
        await Task.Delay(settleDelay);
        await Dispatcher.InvokeAsync(static () => { }, DispatcherPriority.ApplicationIdle);
    }

    protected override Exception? MapActionError(Exception error, UiNode<AutomationPeer>? target) => error switch
    {
        ElementNotEnabledException => UiFallbackInput.Disabled(target?.Label ?? "控件", target?.Entry.Ref ?? ""),
        InvalidOperationException when target is not null => UiFallbackInput.Unsupported(target.Label, target.Entry.Ref!, error.Message),
        _ => null,
    };

    /// <summary>激活：Invoke（按钮，异步投递）→ Toggle → SelectionItem.Select → ExpandCollapse。</summary>
    public override Task<UiActionResult> Click(string reference)
    {
        var before = Snapshot();
        var n = Actionable(reference, before);
        Action? run = n.Pattern<IInvokeProvider>(PatternInterface.Invoke) is { } invoke ? invoke.Invoke
            : n.Pattern<IToggleProvider>(PatternInterface.Toggle) is { } toggle ? toggle.Toggle
            : n.Pattern<ISelectionItemProvider>(PatternInterface.SelectionItem) is { } item ? item.Select
            : n.Pattern<IExpandCollapseProvider>(PatternInterface.ExpandCollapse) is { } ec ? () => ToggleExpand(ec)
            : null;
        if (run is null) throw UiFallbackInput.Unsupported(n.Label, reference);
        return Act(before, n, run);
    }

    private static void ToggleExpand(IExpandCollapseProvider ec)
    {
        if (ec.ExpandCollapseState == ExpandCollapseState.Collapsed) ec.Expand();
        else ec.Collapse();
    }

    /// <summary>填写：文本框 Value（再 UpdateSource）、复选框 Toggle、单选框 Select、下拉框按选项文本、滑块 RangeValue。</summary>
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
            _ => throw UiFallbackInput.Unsupported(n.Label, reference, "只能填写文本框、复选框、单选框、下拉框与滑块"),
        };
    }

    private static string Text(UiNode<AutomationPeer> n, object value) => UiFillValue.Text(n.Label, n.Entry.Ref, value);

    private static void SetText(UiNode<AutomationPeer> n, object value)
    {
        var text = Text(n, value);
        if (n.Pattern<IValueProvider>(PatternInterface.Value) is not { } v) throw UiFallbackInput.Unsupported(n.Label, n.Entry.Ref!);
        if (v.IsReadOnly) throw UiFallbackInput.Unsupported(n.Label, n.Entry.Ref!, "只读");
        v.SetValue(text);
        // @why Value 模式只改 Text；默认 LostFocus 触发的绑定不会写回数据源，需显式 UpdateSource。
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

    /// <summary>下拉框：可编辑时直接写值；否则展开、按选项文本（先精确后忽略大小写）选中、收起。</summary>
    private static void SelectOption(UiNode<AutomationPeer> n, object value)
    {
        var text = Text(n, value);
        if (n.Pattern<IValueProvider>(PatternInterface.Value) is { IsReadOnly: false } editable)
        {
            editable.SetValue(text);
            return;
        }
        if (n.Pattern<IExpandCollapseProvider>(PatternInterface.ExpandCollapse) is not { } ec) throw UiFallbackInput.Unsupported(n.Label, n.Entry.Ref!);
        var wasCollapsed = ec.ExpandCollapseState == ExpandCollapseState.Collapsed;
        if (wasCollapsed) ec.Expand();
        try
        {
            // @why 下拉框的选项在展开（生成容器）之后才出现在子节点中。
            Dispatcher.Invoke(static () => { }, DispatcherPriority.Loaded);
            var options = n.Node.GetChildren() ?? [];
            var match = options.FirstOrDefault(o => o.GetName() == text)
                ?? options.FirstOrDefault(o => string.Equals(o.GetName(), text, StringComparison.OrdinalIgnoreCase));
            if (match?.GetPattern(PatternInterface.SelectionItem) is not ISelectionItemProvider item)
            {
                var names = string.Join("、", options.Select(o => o.GetName()).Where(s => s.Length > 0).Take(20));
                throw UiFallbackInput.Invalid($"{n.Label}没有选项「{text}」；可选：{names}", n.Entry.Ref);
            }
            item.Select();
        }
        finally
        {
            if (wasCollapsed && ec.ExpandCollapseState != ExpandCollapseState.Collapsed) ec.Collapse();
        }
    }

    private static void SetRange(UiNode<AutomationPeer> n, object value)
    {
        var number = UiFillValue.Number(n.Label, n.Entry.Ref, value);
        if (n.Pattern<IRangeValueProvider>(PatternInterface.RangeValue) is not { } r) throw UiFallbackInput.Unsupported(n.Label, n.Entry.Ref!);
        r.SetValue(number);
    }

    /// <summary>按键：ref 给出时先聚焦该控件；Tab 走焦点导航，其余经 InputManager 发送按下 / 抬起，
    /// Enter / Escape 未被处理时再交给默认 / 取消按钮（AccessKeyManager）。</summary>
    public override Task<UiActionResult> Press(string? reference, string key)
    {
        var kind = UiFallbackInput.Key(key);
        var before = Snapshot();
        var target = reference is null ? null : Actionable(reference, before);
        if (target is not null) RefuseSecure(target);
        target?.Owner()?.Focus();
        if (Keyboard.FocusedElement is PasswordBox) throw UiFallbackInput.Secure("当前焦点控件", reference);
        var handled = true;
        return ActKey(before, target, () => handled = ApplyKey(kind), () => handled);
    }

    private async Task<UiActionResult> ActKey(UiSnapshot<AutomationPeer> before, UiNode<AutomationPeer>? target, Action run, Func<bool> handled)
    {
        var result = await Act(before, target, run);
        return handled() ? result : result with { Hint = result.Hint ?? "按键没有被任何控件处理" };
    }

    private static bool ApplyKey(UiKey kind)
    {
        var focused = Keyboard.FocusedElement as UIElement;
        return kind switch
        {
            UiKey.Tab => (focused ?? FirstWindow())?.MoveFocus(new TraversalRequest(FocusNavigationDirection.Next)) ?? false,
            UiKey.ShiftTab => (focused ?? FirstWindow())?.MoveFocus(new TraversalRequest(FocusNavigationDirection.Previous)) ?? false,
            UiKey.Enter => SendKey(focused, Key.Enter) || AccessKey(focused, "\r"),
            UiKey.Escape => SendKey(focused, Key.Escape) || AccessKey(focused, "\u001b"),
            UiKey.Space => SendKey(focused, Key.Space),
            _ => false,
        };
    }

    private static UIElement? FirstWindow() => WpfUiTree.UsableWindows().FirstOrDefault(w => w.IsActive) ?? WpfUiTree.UsableWindows().FirstOrDefault();

    /// <summary>经 InputManager 发送一次按下 + 抬起（目标为键盘焦点）；返回按下是否被处理。</summary>
    private static bool SendKey(UIElement? focused, Key key)
    {
        var target = focused ?? FirstWindow();
        if (target is null || PresentationSource.FromVisual(target) is not { } source) return false;
        var handled = false;
        foreach (var routed in new[] { Keyboard.PreviewKeyDownEvent, Keyboard.KeyDownEvent, Keyboard.PreviewKeyUpEvent, Keyboard.KeyUpEvent })
        {
            var args = new KeyEventArgs(Keyboard.PrimaryDevice, source, Environment.TickCount, key) { RoutedEvent = routed };
            InputManager.Current.ProcessInput(args);
            if (routed == Keyboard.PreviewKeyDownEvent || routed == Keyboard.KeyDownEvent) handled |= args.Handled;
        }
        return handled;
    }

    /// <summary>默认按钮（IsDefault，"\r"）/ 取消按钮（IsCancel，Esc）。</summary>
    private static bool AccessKey(UIElement? focused, string key)
    {
        var scope = (DependencyObject?)focused ?? FirstWindow();
        return scope is not null && AccessKeyManager.IsKeyRegistered(Window.GetWindow(scope) ?? scope, key)
            && AccessKeyManager.ProcessKey(Window.GetWindow(scope) ?? scope, key, false);
    }

    /// <summary>查看方向 → 滚动量（spec/ui-fallback.md 第 2 节：down = 向下翻看更多内容）。</summary>
    private static readonly Dictionary<UiScrollDirection, (ScrollAmount Horizontal, ScrollAmount Vertical)> ScrollAmounts = new()
    {
        [UiScrollDirection.Down] = (ScrollAmount.NoAmount, ScrollAmount.LargeIncrement),
        [UiScrollDirection.Up] = (ScrollAmount.NoAmount, ScrollAmount.LargeDecrement),
        [UiScrollDirection.Right] = (ScrollAmount.LargeIncrement, ScrollAmount.NoAmount),
        [UiScrollDirection.Left] = (ScrollAmount.LargeDecrement, ScrollAmount.NoAmount),
    };

    /// <summary>无方向：滚动到可见（ScrollItem 或 BringIntoView）；有方向：最近的可滚动祖先（含自身）滚动一页。</summary>
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
        var amount = ScrollAmounts[d];
        for (var cur = peer; cur is not null; cur = cur.GetParent())
        {
            if (cur.GetPattern(PatternInterface.Scroll) is IScrollProvider s && CanScroll(s, amount))
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
        else ((peer as UIElementAutomationPeer)?.Owner as FrameworkElement)?.BringIntoView();
    }

    /// <summary>可见文本：名称与值（密码类只给掩码），折叠空白。</summary>
    public override UiReadResult Read(string? reference, int? maxChars)
    {
        var max = Math.Clamp(maxChars ?? UiOutlineFormat.ReadDefault, 1, UiOutlineFormat.ReadMax);
        var parts = new List<string>();
        void Walk(AutomationPeer p)
        {
            if (p.IsOffscreen()) return;
            var secure = WpfUiTree.IsSecure(p);
            parts.Add(p.GetName());
            if (secure)
            {
                if (WpfUiTree.SecureHasValue(p)) parts.Add(UiOutlineFormat.SecureMask);
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
            foreach (var w in WpfUiTree.UsableWindows())
            {
                if (UIElementAutomationPeer.CreatePeerForElement(w) is { } peer) Walk(peer);
            }
        }
        return UiOutlineFormat.ReadText(parts, reference, max);
    }
}
