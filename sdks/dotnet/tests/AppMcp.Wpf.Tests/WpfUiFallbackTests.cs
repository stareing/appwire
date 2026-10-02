using System.ComponentModel;
using System.Runtime.CompilerServices;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Data;
using AppMcp.UiFallback;
using AppMcp.Wpf.Tests.Support;
using AppMcp.Wpf.UiFallback;

[assembly: CollectionBehavior(DisableTestParallelization = true)]

namespace AppMcp.Wpf.Tests;

public sealed class FormVm : INotifyPropertyChanged
{
    private string _name = "";
    /// <summary>默认绑定（UpdateSourceTrigger = LostFocus）：Value 模式写入后需 UpdateSource 才写回。</summary>
    public string Name { get => _name; set { _name = value; On(); } }
    public event PropertyChangedEventHandler? PropertyChanged;
    private void On([CallerMemberName] string? n = null) => PropertyChanged?.Invoke(this, new(n));
}

/// <summary>控件兜底（spec/ui-fallback.md）在真实 WPF 窗口上的行为。</summary>
public class WpfUiFallbackTests
{
    private sealed class Form
    {
        public required Window Window { get; init; }
        public required StackPanel Panel { get; init; }
        public required Button Pay { get; init; }
        public required Button Clear { get; init; }
        public required Button Temp { get; init; }
        public required CheckBox Agree { get; init; }
        public required TextBox Name { get; init; }
        public required PasswordBox Password { get; init; }
        public required ComboBox Color { get; init; }
        public required FormVm Vm { get; init; }
        public List<string> Log { get; } = [];
    }

    private static async Task<Form> ShowForm()
    {
        var vm = new FormVm();
        var panel = new StackPanel();
        var form = new Form
        {
            Window = new Window { Title = "Probe", Width = 420, Height = 560, DataContext = vm, Content = panel },
            Panel = panel,
            Pay = new Button { Content = "Pay now" },
            Clear = new Button { Content = "Clear" },
            Temp = new Button { Content = "Remove me" },
            Agree = new CheckBox { Content = "Agree" },
            Name = new TextBox(),
            Password = new PasswordBox { Password = "s3cret" },
            Color = new ComboBox { ItemsSource = new[] { "Red", "Green", "Blue" } },
            Vm = vm,
        };
        form.Pay.Click += (_, _) => form.Log.Add("pay");
        form.Clear.Click += (_, _) => form.Log.Add("clear");
        form.Temp.Click += (_, _) => panel.Children.Remove(form.Temp);
        AutomationPropertiesName(form.Name, "Name");
        AutomationPropertiesName(form.Password, "Password");
        AutomationPropertiesName(form.Color, "Color");
        form.Name.SetBinding(TextBox.TextProperty, new Binding(nameof(FormVm.Name)));
        var locked = new Button { Content = "Locked", IsEnabled = false };
        var hidden = new Button { Content = "Collapsed btn", Visibility = Visibility.Collapsed };
        var openDialog = new Button { Content = "Open dialog" };
        openDialog.Click += (_, _) =>
        {
            var ok = new Button { Content = "OK", IsDefault = true };
            var cancel = new Button { Content = "Cancel", IsCancel = true };
            var dialog = new Window { Title = "Confirm", Owner = form.Window, Width = 240, Height = 160, Content = new StackPanel { Children = { ok, cancel } } };
            ok.Click += (_, _) => { form.Log.Add("ok"); dialog.DialogResult = true; };
            form.Log.Add($"dialog:{dialog.ShowDialog()}");
        };
        var list = new ListBox { Height = 80, ItemsSource = Enumerable.Range(0, 50).Select(i => $"Item {i}").ToList() };
        AutomationPropertiesName(list, "Items");
        foreach (UIElement e in new UIElement[] { form.Pay, locked, form.Clear, form.Agree, form.Name, form.Password, form.Color, hidden, form.Temp, openDialog, list })
        {
            panel.Children.Add(e);
        }
        await WpfTestApp.Show(form.Window);
        return form;
    }

    private static void AutomationPropertiesName(DependencyObject d, string name) => System.Windows.Automation.AutomationProperties.SetName(d, name);

    private static WpfUiInspector Inspector() => new("ui", 60, TimeSpan.Zero, WpfViewTools.DeclaredTools);

    private static UiOutlineItem Item(UiOutlineResult o, string name) =>
        o.Items.FirstOrDefault(i => i.Name == name) ?? throw new Xunit.Sdk.XunitException($"大纲中没有「{name}」：\n{o.Text}");

    private static async Task<ToolCallException> Fails(Func<Task> act, string? reason)
    {
        var e = await Assert.ThrowsAsync<ToolCallException>(act);
        Assert.Equal(ToolErrorKind.InvalidInput, e.Kind);
        var details = e.Details as IDictionary<string, object?>;
        Assert.Equal(reason, details is not null && details.TryGetValue("reason", out var r) ? r : null);
        return e;
    }

    [Fact]
    public Task OutlineListsVisibleControlsAndMasksPasswords() => WpfTestApp.Run(async () =>
    {
        var f = await ShowForm();
        try
        {
            var o = Inspector().Outline(null, null, null);
            Assert.Contains("窗口「Probe」", o.Text);
            Assert.Contains("按钮「Pay now」", o.Text);
            Assert.Contains("按钮「Locked」 disabled", o.Text);
            Assert.Contains("复选框「Agree」 unchecked", o.Text);
            Assert.Contains("密码框「Password」= \"••••\"", o.Text);
            Assert.DoesNotContain("s3cret", o.Text);
            Assert.DoesNotContain("Collapsed btn", o.Text);
            Assert.Equal("combobox", Item(o, "Color").Role);
            Assert.Equal("窗口「Probe」", Item(o, "Pay now").Group);
            Assert.Contains("Item 0", o.Text);
            Assert.DoesNotContain("Item 40", o.Text); // 屏外（滚动区外）不列出

            var limited = Inspector().Outline(null, null, 2);
            Assert.Equal(2, limited.Items.Count);
            Assert.Equal(o.Total - 2, limited.Remaining);
            var q = Inspector().Outline("agree", null, null);
            Assert.Equal("Agree", Assert.Single(q.Items).Name);
        }
        finally { f.Window.Close(); }
    });

    [Fact]
    public Task ClickInvokesButtonsAndTogglesCheckBoxes() => WpfTestApp.Run(async () =>
    {
        var f = await ShowForm();
        try
        {
            var ui = Inspector();
            var o = ui.Outline(null, null, null);
            await ui.Click(Item(o, "Pay now").Ref);
            Assert.Equal(["pay"], f.Log); // Invoke 异步投递，等调度器空闲后已执行
            var agree = Item(o, "Agree").Ref;
            var r = await ui.Click(agree);
            Assert.True(f.Agree.IsChecked);
            Assert.Contains($"{agree} Agree 变为 checked", r.Changes);
            await Fails(() => ui.Click(Item(o, "Locked").Ref), "TOOL_DISABLED");
        }
        finally { f.Window.Close(); }
    });

    [Fact]
    public Task FillWritesTextThroughBindingSelectsAndToggles() => WpfTestApp.Run(async () =>
    {
        var f = await ShowForm();
        try
        {
            var ui = Inspector();
            var o = ui.Outline(null, null, null);
            var r = await ui.Fill(Item(o, "Name").Ref, "Alice");
            Assert.Equal("Alice", f.Name.Text);
            Assert.Equal("Alice", f.Vm.Name); // LostFocus 绑定也已写回
            Assert.Contains(r.Changes, c => c.Contains("值变为 \"Alice\""));
            await ui.Fill(Item(o, "Color").Ref, "blue");
            Assert.Equal("Blue", f.Color.SelectedItem);
            Assert.Equal("Blue", Item(ui.Outline(null, null, null), "Color").Value);
            await ui.Fill(Item(o, "Agree").Ref, true);
            Assert.True(f.Agree.IsChecked);
            await ui.Fill(Item(o, "Agree").Ref, true); // 已是目标状态
            Assert.True(f.Agree.IsChecked);
            await Fails(() => ui.Fill(Item(o, "Color").Ref, "Purple"), null);
            await Fails(() => ui.Fill(Item(o, "Pay now").Ref, "x"), "unsupported");
        }
        finally { f.Window.Close(); }
    });

    [Fact]
    public Task PasswordBoxIsNeverWrittenOrRead() => WpfTestApp.Run(async () =>
    {
        var f = await ShowForm();
        try
        {
            var ui = Inspector();
            var pwd = Item(ui.Outline(null, null, null), "Password").Ref;
            await Fails(() => ui.Fill(pwd, "stolen"), "secure");
            await Fails(() => ui.Press(pwd, "Enter"), "secure");
            Assert.Equal("s3cret", f.Password.Password);
            Assert.Equal("Password ••••", ui.Read(pwd, null).Text);
            Assert.DoesNotContain("s3cret", ui.Read(null, null).Text);
        }
        finally { f.Window.Close(); }
    });

    [Fact]
    public Task StaleAndUnknownRefsFail() => WpfTestApp.Run(async () =>
    {
        var f = await ShowForm();
        try
        {
            var ui = Inspector();
            var temp = Item(ui.Outline(null, null, null), "Remove me").Ref;
            await ui.Click(temp);
            var e = await Fails(() => ui.Click(temp), null);
            Assert.Contains($"引用 {temp} 已失效", e.Message);
            await Fails(() => ui.Click("e9999"), null);
        }
        finally { f.Window.Close(); }
    });

    [Fact]
    public Task ModalDialogHidesOwnerAndEscapeCancels() => WpfTestApp.Run(async () =>
    {
        var f = await ShowForm();
        try
        {
            var ui = Inspector();
            var o = ui.Outline(null, null, null);
            var pay = Item(o, "Pay now").Ref;
            var opened = await ui.Click(Item(o, "Open dialog").Ref);
            // ShowDialog 在嵌套消息循环中：此处已在对话框打开期间。
            Assert.Contains(opened.Changes, c => c.StartsWith("新增对话框「Confirm」") && c.Contains("含 2 个可交互元素"));
            var withDialog = ui.Outline(null, null, null);
            Assert.DoesNotContain("Pay now", withDialog.Text);
            Assert.Equal("对话框「Confirm」", Item(withDialog, "OK").Group);
            await Fails(() => ui.Click(pay), "hidden");

            await ui.Press(null, "Escape"); // 取消按钮（IsCancel）
            Assert.Contains("dialog:False", f.Log);
            Assert.Contains("Pay now", ui.Outline(null, null, null).Text);

            await ui.Click(Item(ui.Outline(null, null, null), "Open dialog").Ref);
            await ui.Press(null, "Enter"); // 默认按钮（IsDefault）
            Assert.Equal(["dialog:False", "ok", "dialog:True"], f.Log);
            await Fails(() => ui.Press(null, "F13"), null);
        }
        finally { f.Window.Close(); }
    });

    [Fact]
    public Task PressTabMovesFocusAndSpaceActivates() => WpfTestApp.Run(async () =>
    {
        var f = await ShowForm();
        try
        {
            var ui = Inspector();
            var o = ui.Outline(null, null, null);
            await ui.Press(Item(o, "Pay now").Ref, "Tab");
            Assert.True(f.Clear.IsKeyboardFocused);
            await ui.Press(Item(o, "Pay now").Ref, "Space");
            Assert.Equal(["pay"], f.Log);
        }
        finally { f.Window.Close(); }
    });

    [Fact]
    public Task ScrollPagesAndReadText() => WpfTestApp.Run(async () =>
    {
        var f = await ShowForm();
        try
        {
            var ui = Inspector();
            var o = ui.Outline(null, null, null);
            var items = Item(o, "Item 0").Ref;
            var r = await ui.Scroll(items, "down");
            Assert.NotEmpty(r.Changes);
            Assert.DoesNotContain("「Item 0」", ui.Outline(null, null, null).Text);
            await Fails(() => ui.Scroll(items, "sideways"), null);
            Assert.Equal("Pay now", ui.Read(Item(o, "Pay now").Ref, null).Text);
            var shortRead = ui.Read(null, 5);
            Assert.True(shortRead.Truncated);
            Assert.EndsWith("…", shortRead.Text);
        }
        finally { f.Window.Close(); }
    });

    [Fact]
    public Task DeclaredToolsAndVisibilityGating() => WpfTestApp.Run(async () =>
    {
        var f = await ShowForm();
        using var client = AppMcpClient.Create(new AppMcpClientOptions
        {
            AppId = "wpf-fallback-test",
            AppName = "兜底测试",
            HostUrl = "ws://127.0.0.1:9/app", // 不启动、不连接
            Dispatcher = null,
        });
        try
        {
            using var clear = client.RegisterTool("cart.clear", "清空", (_, _) => Task.FromResult<object?>(null));
            using var binding = WpfViewTools.Bind(f.Clear, clear);
            using var fallback = WpfUiFallback.Enable(client);
            var o = fallback.Inspector.Outline(null, null, null);
            Assert.Equal("cart.clear", Item(o, "Clear").Declared);
            Assert.Contains("按钮「Clear」 [已声明：cart.clear]", o.Text);
            Assert.NotNull(o.Hint);
            var r = await fallback.Inspector.Click(Item(o, "Clear").Ref);
            Assert.Contains("cart.clear", r.Hint);

            Assert.True(fallback.Enabled);
            f.Window.WindowState = WindowState.Minimized;
            Assert.False(fallback.Enabled);
            f.Window.WindowState = WindowState.Normal;
            Assert.True(fallback.Enabled);
            f.Window.Hide();
            Assert.False(fallback.Enabled);
            f.Window.Show();
            Assert.True(fallback.Enabled);
        }
        finally { f.Window.Close(); }
    });
}
