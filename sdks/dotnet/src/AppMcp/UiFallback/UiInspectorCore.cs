using System.Globalization;
using System.Text.Json;

namespace AppMcp.UiFallback;

/// <summary>兜底工具的执行引擎接口（spec/ui-fallback.md 第 2 节）：所有方法都在 UI 线程上调用。</summary>
internal interface IUiInspector
{
    string Prefix { get; }
    int MaxItems { get; }
    UiOutlineResult Outline(string? query, string? within, int? limit);
    Task<UiActionResult> Click(string reference);
    Task<UiActionResult> Fill(string reference, object value);
    Task<UiActionResult> Press(string? reference, string key);
    Task<UiActionResult> Scroll(string reference, string? direction);
    UiReadResult Read(string? reference, int? maxChars);
}

/// <summary>
/// 与 UI 框架无关的执行骨架（spec/ui-fallback.md 7.1 / 7.2）：按引用取控件并核对（失效 / 不可见 / 禁用 / 密码类）、
/// 执行动作后等界面稳定、前后对比出变化摘要。具体框架实现收集、启用判断、动作与"等稳定"。
/// </summary>
internal abstract class UiInspectorCore<TNode>(string prefix, int maxItems) : IUiInspector where TNode : class
{
    protected UiRefRegistry<TNode> Refs { get; } = new();

    public string Prefix => prefix;
    public int MaxItems => maxItems;

    public void Clear() => Refs.Clear();

    /// <summary>收集 <paramref name="start"/>（缺省为全部可见窗口）下可见的条目。</summary>
    protected abstract UiSnapshot<TNode> Collect(TNode? start);

    /// <summary>控件仍在界面上（已加载、所在窗口未关闭），但可能不可见。</summary>
    protected abstract bool IsAlive(TNode node);

    protected abstract string NameOf(TNode node);

    protected abstract bool IsEnabled(TNode node);

    /// <summary>动作后等界面稳定（让出调度器、短暂延迟）。</summary>
    protected abstract Task Settle();

    /// <summary>把框架在动作中抛出的异常转换为兜底错误；返回 null 表示原样抛出。</summary>
    protected virtual Exception? MapActionError(Exception error, UiNode<TNode>? target) => null;

    protected UiSnapshot<TNode> Snapshot() => Collect(null);

    /// <summary>按引用在当前界面中取控件（spec/ui-fallback.md 7.1）。</summary>
    protected UiNode<TNode> Resolve(string reference, UiSnapshot<TNode> snap)
    {
        if (snap.ByRef.TryGetValue(reference, out var node)) return node;
        if (Alive(reference) is { } alive)
        {
            throw UiFallbackInput.Hidden($"{reference} {UiOutlineFormat.Collapse(NameOf(alive))}", reference, prefix);
        }
        throw UiFallbackInput.Stale(reference, prefix);
    }

    /// <summary>引用对应的控件仍在界面上（可能不可见）。</summary>
    protected TNode? Alive(string reference) => Refs.Target(reference) is { } n && IsAlive(n) ? n : null;

    protected UiNode<TNode> Actionable(string reference, UiSnapshot<TNode> snap)
    {
        var n = Resolve(reference, snap);
        if (!IsEnabled(n.Node)) throw UiFallbackInput.Disabled(n.Label, reference);
        return n;
    }

    protected static void RefuseSecure(UiNode<TNode> n)
    {
        if (n.Secure) throw UiFallbackInput.Secure(n.Label, n.Entry.Ref);
    }

    protected async Task<UiActionResult> Act(UiSnapshot<TNode> before, UiNode<TNode>? target, Action run)
    {
        try
        {
            run();
        }
        catch (Exception e) when (e is not ToolCallException && MapActionError(e, target) is { } mapped)
        {
            throw mapped;
        }
        await Settle();
        var after = Snapshot();
        return new UiActionResult(UiOutlineFormat.Diff(before.Entries, after.Entries, prefix))
        {
            Hint = target?.Entry.Declared is { } d ? $"该元素已声明为工具 {d}，下次可直接调用" : null,
        };
    }

    protected static Task<UiActionResult> Unchanged(string? hint = null) => Task.FromResult(new UiActionResult([]) { Hint = hint });

    public UiOutlineResult Outline(string? query, string? within, int? limit)
    {
        var n = Math.Clamp(limit ?? maxItems, 1, UiOutlineFormat.LimitMax);
        var snap = Snapshot();
        if (within is null) return UiOutlineFormat.Render(snap.Entries, query, n);
        var scope = Resolve(within, snap);
        return UiOutlineFormat.Render(Collect(scope.Node).Entries, query, n);
    }

    public abstract Task<UiActionResult> Click(string reference);
    public abstract Task<UiActionResult> Fill(string reference, object value);
    public abstract Task<UiActionResult> Press(string? reference, string key);
    public abstract Task<UiActionResult> Scroll(string reference, string? direction);
    public abstract UiReadResult Read(string? reference, int? maxChars);
}

/// <summary>兜底工具的注册（名称、说明、schema、注解；spec/ui-fallback.md 第 2 节），各 UI 框架绑定共用。</summary>
internal static class UiFallbackTools
{
    /// <summary>
    /// 在 <paramref name="scope"/> 下注册六个兜底工具（<see cref="ToolSurface.View"/>、初始禁用）。
    /// <paramref name="onUi"/> 负责切到 UI 线程并核对启用状态后执行。
    /// </summary>
    public static List<ToolRegistration> Register(
        ToolScope scope,
        IUiInspector ui,
        Func<Func<Task<object?>>, Task<object?>> onUi)
    {
        var prefix = ui.Prefix;
        ToolRegistration Add(string name, string title, string description, string schema, bool readOnly, Func<JsonElement, Task<object?>> run) =>
            scope.RegisterTool($"{prefix}.{name}", description, (args, _) => onUi(() => run(args)), new ToolOptions
            {
                Title = title,
                InputSchemaJson = schema,
                Risk = readOnly ? ToolRisk.Read : ToolRisk.Write,
                Annotations = new ToolAnnotations { Title = title, ReadOnlyHint = readOnly },
                Surface = ToolSurface.View,
                Enabled = false,
            });

        static Task<object?> Done(object result) => Task.FromResult<object?>(result);

        return
        [
            Add("outline", "界面控件大纲",
                "兜底能力：列出当前窗口可见的可交互控件（按钮、输入框、复选框等），每行一个，带引用 eN，按窗口 / 对话框 / 分组归类。" +
                "应用已有对应的业务工具或标注 [已声明：…] 时请优先使用那些工具。引用用于 click / fill / press / scroll / read。",
                UiFallbackInput.Schemas.Outline(ui.MaxItems), readOnly: true,
                a => Done(ui.Outline(UiFallbackInput.String(a, "query"), UiFallbackInput.Ref(a, "within", required: false), UiFallbackInput.Int(a, "limit")))),
            Add("click", "点击控件",
                $"兜底能力：激活 {prefix}.outline 中的控件（点击按钮、切换复选框、选中选项、展开 / 收起），返回界面变化摘要。",
                UiFallbackInput.Schemas.Ref, readOnly: false,
                async a => await ui.Click(UiFallbackInput.Ref(a, "ref")!)),
            Add("fill", "填写控件",
                "兜底能力：填写文本框（会写回数据绑定）；复选框 / 单选框传 true / false；下拉框传选项文本；滑块传数字。密码类控件不支持。返回界面变化摘要。",
                UiFallbackInput.Schemas.Fill, readOnly: false,
                async a => await ui.Fill(UiFallbackInput.Ref(a, "ref")!, UiFallbackInput.Value(a))),
            Add("press", "按键",
                "兜底能力：按键（ref 缺省为当前焦点控件）：Enter（激活 / 默认按钮）、Escape（取消按钮 / 关闭）、Tab / Shift+Tab（移动焦点）、Space（激活）。" +
                "输入文本请用 fill。返回界面变化摘要。",
                UiFallbackInput.Schemas.Press, readOnly: false,
                async a => await ui.Press(UiFallbackInput.Ref(a, "ref", required: false),
                    UiFallbackInput.String(a, "key") ?? throw UiFallbackInput.Invalid("缺少参数 key"))),
            Add("scroll", "滚动",
                "兜底能力：无 direction 时把控件滚动到可见；direction 为 up / down / left / right 时滚动该控件所在的滚动区一页（down = 向下翻看更多内容）。返回界面变化摘要。",
                UiFallbackInput.Schemas.Scroll, readOnly: false,
                async a => await ui.Scroll(UiFallbackInput.Ref(a, "ref")!, UiFallbackInput.String(a, "direction"))),
            Add("read", "读取控件文本",
                $"兜底能力：读取控件的可见文本（折叠空白，默认最多 {UiOutlineFormat.ReadDefault} 字）；ref 缺省为全部窗口。",
                UiFallbackInput.Schemas.Read, readOnly: true,
                a => Done(ui.Read(UiFallbackInput.Ref(a, "ref", required: false), UiFallbackInput.Int(a, "maxChars")))),
        ];
    }
}

/// <summary>支持的按键（spec/ui-fallback.md 2.1）。</summary>
internal enum UiKey { Enter, Escape, Tab, ShiftTab, Space }

/// <summary>查看方向（spec/ui-fallback.md 第 2 节：<see cref="Down"/> = 向下翻看更多内容）。</summary>
internal enum UiScrollDirection { Up, Down, Left, Right }

/// <summary>填写值的转换（文本 / 布尔 / 数字），WPF / WinUI 共用。</summary>
internal static class UiFillValue
{
    public static string Text(string label, string? reference, object value) => value switch
    {
        string s => s,
        double d => d.ToString(CultureInfo.InvariantCulture),
        _ => throw UiFallbackInput.Invalid($"{label}需要文本值", reference),
    };

    public static bool Bool(string label, string? reference, object value) =>
        value as bool? ?? throw UiFallbackInput.Invalid($"{label}需要 true / false", reference);

    public static double Number(string label, string? reference, object value) => value switch
    {
        double d => d,
        string s when double.TryParse(s, NumberStyles.Float, CultureInfo.InvariantCulture, out var d) => d,
        _ => throw UiFallbackInput.Invalid($"{label}需要数字", reference),
    };
}
