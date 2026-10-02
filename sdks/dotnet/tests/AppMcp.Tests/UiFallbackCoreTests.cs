using System.Text.Json;
using AppMcp.UiFallback;

namespace AppMcp.Tests;

/// <summary>
/// 控件兜底中与 UI 框架无关的部分（spec/ui-fallback.md 第 3–7 节）：树收集、引用、执行前核对、变化摘要、参数解析。
/// WPF / WinUI 共用这些逻辑；这里用假控件树在任意平台上验证。
/// </summary>
public class UiFallbackCoreTests
{
    /// <summary>假控件：测试直接改字段模拟界面变化。</summary>
    private sealed class Node(string? role, string name = "", UiEntryKind kind = UiEntryKind.Item)
    {
        public string? Role { get; set; } = role;
        public string Name { get; set; } = name;
        public UiEntryKind Kind { get; } = kind;
        public string? Value { get; set; }
        public List<string> States { get; set; } = [];
        public bool Secure { get; init; }
        public bool Hidden { get; set; }
        public bool Enabled { get; set; } = true;
        public bool Attached { get; set; } = true;
        public string? Declared { get; init; }
        public int Level { get; init; } = 2;
        public List<Node> Children { get; } = [];
        public Action OnClick { get; set; } = () => { };
    }

    private sealed class Inspector(Func<IEnumerable<Node>> roots) : UiInspectorCore<Node>("ui", 60)
    {
        public int Settles { get; private set; }

        protected override UiSnapshot<Node> Collect(Node? start)
        {
            var tree = new UiTreeBuilder<Node>();
            void Walk(Node n)
            {
                if (n.Hidden) return;
                switch (n.Role, n.Kind)
                {
                    case (null, _):
                        break;
                    case (_, UiEntryKind.Heading):
                        tree.Heading(n, n.Level, n.Name);
                        break;
                    case (_, UiEntryKind.Container):
                        tree.BeginContainer(n, n.Role, n.Name);
                        n.Children.ForEach(Walk);
                        tree.EndContainer();
                        return;
                    default:
                        tree.Item(n, n.Role, n.Name, n.Secure);
                        return;
                }
                n.Children.ForEach(Walk);
            }
            if (start is not null) Walk(start);
            else foreach (var r in roots()) Walk(r);
            return tree.Build(Refs, (n, role, secure, kind) => new UiEntryDetails(
                secure ? (n.Value is { Length: > 0 } ? UiOutlineFormat.SecureMask : null) : n.Value, n.States, false, n.Declared));
        }

        protected override bool IsAlive(Node node) => node.Attached;
        protected override string NameOf(Node node) => node.Name;
        protected override bool IsEnabled(Node node) => node.Enabled;

        protected override Task Settle()
        {
            Settles++;
            return Task.CompletedTask;
        }

        protected override Exception? MapActionError(Exception error, UiNode<Node>? target) =>
            error is InvalidOperationException && target is not null ? UiFallbackInput.Unsupported(target.Label, target.Entry.Ref!, error.Message) : null;

        public override Task<UiActionResult> Click(string reference)
        {
            var before = Snapshot();
            var n = Actionable(reference, before);
            return Act(before, n, n.Node.OnClick);
        }

        public override Task<UiActionResult> Fill(string reference, object value)
        {
            var before = Snapshot();
            var n = Actionable(reference, before);
            RefuseSecure(n);
            var text = UiFillValue.Text(n.Label, reference, value);
            return Act(before, n, () => n.Node.Value = text);
        }

        public override Task<UiActionResult> Press(string? reference, string key) => throw new NotSupportedException();

        public override Task<UiActionResult> Scroll(string reference, string? direction)
        {
            UiFallbackInput.Direction(direction);
            var before = Snapshot();
            if (before.ByRef.ContainsKey(reference)) return Unchanged();
            var node = Alive(reference) ?? throw UiFallbackInput.Stale(reference, Prefix);
            return Act(before, null, () => node.Hidden = false);
        }

        public override UiReadResult Read(string? reference, int? maxChars) =>
            UiOutlineFormat.ReadText(Snapshot().Entries.Select(e => e.Name), reference, maxChars ?? UiOutlineFormat.ReadDefault);
    }

    private readonly Node _window = new("window", "商城", UiEntryKind.Container);
    private readonly Node _clear = new("button", "清空") { Declared = "cart.clear" };
    private readonly Node _pay = new("button", "结算") { Enabled = false, States = ["disabled"] };
    private readonly Node _note = new("textbox", "备注") { Value = "尽快" };
    private readonly Node _password = new("textbox", "支付密码") { Value = "123456", Secure = true };
    private readonly Inspector _ui;

    public UiFallbackCoreTests()
    {
        var group = new Node(null);
        group.Children.AddRange([new Node("heading", "支付", UiEntryKind.Heading), _note, _password]);
        _window.Children.AddRange([_clear, _pay, group]);
        _ui = new Inspector(() => [_window]);
    }

    private static Dictionary<string, object?> Details(Func<Task> action)
    {
        var e = Assert.ThrowsAsync<ToolCallException>(action).GetAwaiter().GetResult();
        Assert.Equal(ToolErrorKind.InvalidInput, e.Kind);
        return e.Details as Dictionary<string, object?> ?? [];
    }

    [Fact]
    public void OutlineGroupsHeadingsAndMasksSecureValues()
    {
        var o = _ui.Outline(null, null, null);
        Assert.Equal(
            "» e1 窗口「商城」\n  e2 按钮「清空」 [已声明：cart.clear]\n  e3 按钮「结算」 disabled\n  ## 支付\n  e4 输入框「备注」= \"尽快\"\n  e5 密码框「支付密码」= \"••••\"",
            o.Text);
        Assert.Equal("窗口「商城」 › 支付", o.Items[2].Group);
        Assert.DoesNotContain("123456", o.Text);
        // 引用在实例内稳定；within 只列出该分组
        Assert.Equal(o.Text, _ui.Outline(null, null, null).Text);
        Assert.Equal(4, _ui.Outline(null, "e1", null).Total);
    }

    [Fact]
    public async Task ActionsReturnChangesAndDeclaredHint()
    {
        _ui.Outline(null, null, null);
        _clear.OnClick = () =>
        {
            _note.Value = null;
            _pay.Enabled = true;
            _pay.States = [];
        };
        var r = await _ui.Click("e2");
        Assert.Equal(["e3 结算 不再 disabled", "e4 备注 值已清空"], r.Changes);
        Assert.Equal("该元素已声明为工具 cart.clear，下次可直接调用", r.Hint);
        Assert.Equal(1, _ui.Settles);
        Assert.Equal(["e4 备注 值变为 \"尽快发货\""], (await _ui.Fill("e4", "尽快发货")).Changes);
        Assert.Null((await _ui.Fill("e4", 3.0)).Hint);
        Assert.Equal("3", _note.Value);
    }

    [Fact]
    public void PreflightRejectsStaleHiddenDisabledAndSecure()
    {
        _ui.Outline(null, null, null);
        Assert.Equal("TOOL_DISABLED", Details(() => _ui.Click("e3"))["reason"]);
        Assert.Equal("secure", Details(() => _ui.Fill("e5", "x"))["reason"]);
        Assert.Equal("123456", _password.Value);
        _clear.Hidden = true;
        var hidden = Details(() => _ui.Click("e2"));
        Assert.Equal("hidden", hidden["reason"]);
        _clear.Attached = false;
        var stale = Details(() => _ui.Click("e2"));
        Assert.Equal("e2", stale["ref"]);
        Assert.False(stale.ContainsKey("reason"));
        Assert.Equal("e99", Details(() => _ui.Click("e99"))["ref"]);
        // 动作中的框架异常转换为 unsupported
        _note.Value = "x";
        var unsupported = new Node("button", "坏按钮") { OnClick = () => throw new InvalidOperationException("不支持") };
        _window.Children.Add(unsupported);
        var r = _ui.Outline(null, null, null).Items.Single(i => i.Name == "坏按钮").Ref;
        Assert.Equal("unsupported", Details(() => _ui.Click(r))["reason"]);
    }

    [Fact]
    public async Task ScrollIntoViewKeepsRefAndClearDropsRefs()
    {
        _ui.Outline(null, null, null);
        Assert.Empty((await _ui.Scroll("e2", null)).Changes);
        _clear.Hidden = true;
        Assert.Equal(["新增按钮「清空」(e2)"], (await _ui.Scroll("e2", null)).Changes);
        Assert.Equal("direction 应为 up / down / left / right", (await Assert.ThrowsAsync<ToolCallException>(() => _ui.Scroll("e2", "sideways"))).Message);
        _ui.Clear();
        Assert.Equal("e2", Details(() => _ui.Click("e2"))["ref"]);
        Assert.Contains("e7 按钮「清空」", _ui.Outline(null, null, null).Text);
    }

    [Fact]
    public void ParsesKeysDirectionsAndValues()
    {
        Assert.Equal(UiKey.ShiftTab, UiFallbackInput.Key("shift+TAB"));
        Assert.Equal(UiKey.Space, UiFallbackInput.Key(" "));
        Assert.Equal(UiKey.Escape, UiFallbackInput.Key(" Esc "));
        Assert.Contains("支持 Enter", Assert.Throws<ToolCallException>(() => UiFallbackInput.Key("F5")).Message);
        Assert.Equal(UiScrollDirection.Left, UiFallbackInput.Direction("left"));
        Assert.Null(UiFallbackInput.Direction(null));
        Assert.Equal(2.5, UiFillValue.Number("滑块", "e1", "2.5"));
        Assert.True(UiFillValue.Bool("复选框", "e1", true));
        Assert.Throws<ToolCallException>(() => UiFillValue.Bool("复选框", "e1", "true"));
        Assert.Throws<ToolCallException>(() => UiFillValue.Text("输入框", "e1", true));
        using var doc = JsonDocument.Parse("{\"value\": false}");
        Assert.Equal(false, UiFallbackInput.Value(doc.RootElement));
    }

    [Fact]
    public void ReadTextCollapsesDedupesAndTruncates()
    {
        Assert.Equal(new UiReadResult("root", "确定 取消", false), UiOutlineFormat.ReadText(["确定", " 确定 ", null, "取消"], null, 100));
        Assert.Equal(new UiReadResult("e1", "确…", true), UiOutlineFormat.ReadText(["确定 取消"], "e1", 2));
    }

    [Fact]
    public void DeclaredToolsAreWeakAndRemovable()
    {
        var registry = new UiDeclaredTools();
        var element = new object();
        registry.Declare(element, ["a", "b", "a"]);
        Assert.Equal("a, b", registry.Of(element));
        registry.Undeclare(element, ["a"]);
        Assert.Equal("b", registry.Of(element));
        registry.Undeclare(element, ["b"]);
        Assert.Null(registry.Of(element));
        Assert.Null(registry.Of(new object()));
    }

    [Fact]
    public void RefRegistryDoesNotKeepNodesAlive()
    {
        var refs = new UiRefRegistry<object>();
        var weak = Register(refs);
        GC.Collect();
        GC.WaitForPendingFinalizers();
        GC.Collect();
        Assert.False(weak.TryGetTarget(out _));
        Assert.Null(refs.Target("e1"));
        refs.Prune();
        Assert.Equal("e2", refs.RefOf(new object()));
    }

    [System.Runtime.CompilerServices.MethodImpl(System.Runtime.CompilerServices.MethodImplOptions.NoInlining)]
    private static WeakReference<object> Register(UiRefRegistry<object> refs)
    {
        var node = new object();
        Assert.Equal("e1", refs.RefOf(node));
        Assert.Equal("e1", refs.RefOf(node));
        Assert.Same(node, refs.Target("e1"));
        return new WeakReference<object>(node);
    }
}
