using AppMcp.UiFallback;

namespace AppMcp.Tests;

/// <summary>控件兜底的大纲与变化摘要格式（spec/ui-fallback.md 第 4、7 节），与 UI 框架无关。</summary>
public class UiOutlineFormatTests
{
    private static UiEntry Window(string reference) => new()
    {
        Kind = UiEntryKind.Container, Key = reference, Ref = reference, Role = "window", Label = "窗口", Name = "商城",
    };

    private static UiEntry Item(string reference, string role, string name, string? value = null, string[]? states = null,
        bool required = false, string? declared = null, int depth = 1, string[]? containers = null) => new()
    {
        Kind = UiEntryKind.Item, Key = reference, Ref = reference, Role = role, Label = UiOutlineFormat.Label(role), Name = name,
        Value = value, States = states ?? [], Required = required, Declared = declared, Depth = depth, Chain = [0],
        Containers = containers ?? ["e1"],
    };

    [Fact]
    public void RendersLinesItemsAndHints()
    {
        var entries = new List<UiEntry>
        {
            Window("e1"),
            Item("e2", "button", "清空", declared: "cart.clear"),
            Item("e3", "button", "结算", states: ["disabled"]),
            Item("e4", "textbox", "备注", value: "尽快", required: true),
        };
        var o = UiOutlineFormat.Render(entries, null, 60);
        Assert.Equal(
            "» e1 窗口「商城」\n  e2 按钮「清空」 [已声明：cart.clear]\n  e3 按钮「结算」 disabled\n  e4 输入框「备注」= \"尽快\" (必填)",
            o.Text);
        Assert.Equal(3, o.Total);
        Assert.Null(o.Remaining);
        Assert.NotNull(o.Hint);
        Assert.Equal("窗口「商城」", o.Items[0].Group);
        Assert.Equal(["disabled"], o.Items[1].States!);

        var limited = UiOutlineFormat.Render(entries, null, 1);
        Assert.Equal(2, limited.Remaining);
        Assert.EndsWith("…另有 2 个元素未列出，可用 query 或 within 缩小范围", limited.Text);
        Assert.Equal("（没有与「付款」匹配的可交互元素）", UiOutlineFormat.Render(entries, "付款", 60).Text);
        Assert.Equal("（没有可见的可交互元素）", UiOutlineFormat.Render([], null, 60).Text);
    }

    [Fact]
    public void DiffReportsChangesAddedGroupsAndRemovals()
    {
        var before = new List<UiEntry> { Window("e1"), Item("e2", "checkbox", "同意", states: ["unchecked"]), Item("e3", "button", "删除") };
        var dialog = new UiEntry { Kind = UiEntryKind.Container, Key = "e9", Ref = "e9", Role = "dialog", Label = "对话框", Name = "确认", Depth = 1, Chain = [0], Containers = ["e1"] };
        var after = new List<UiEntry>
        {
            Window("e1"),
            Item("e2", "checkbox", "同意", states: ["checked", "focused"]),
            dialog,
            Item("e10", "button", "确定", depth: 2, containers: ["e1", "e9"]),
            Item("e11", "button", "取消", depth: 2, containers: ["e1", "e9"]),
        };
        var changes = UiOutlineFormat.Diff(before, after, "ui");
        Assert.Equal(
            [
                "e2 同意 变为 checked",
                "新增对话框「确认」(e9)，含 2 个可交互元素（可用 ui.outline({ within: \"e9\" }) 查看）",
                "按钮「删除」(e3) 已消失",
            ],
            changes);
    }

    [Fact]
    public void TruncatesWithEllipsis()
    {
        Assert.Equal("abc", UiOutlineFormat.Truncate("abc", 3));
        Assert.Equal("ab…", UiOutlineFormat.Truncate("abcd", 3));
        Assert.Equal("a b", UiOutlineFormat.Collapse("  a \n b "));
        Assert.Matches(UiOutlineFormat.RefPattern, "e12");
        Assert.DoesNotMatch(UiOutlineFormat.RefPattern, "e0");
    }
}
