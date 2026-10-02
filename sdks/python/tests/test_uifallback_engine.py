"""进程内控件兜底引擎（spec/ui-fallback.md）：格式、参数、引用、遮挡、各工具与上限。用假界面，不需要 Qt。"""

from __future__ import annotations

import asyncio
from collections.abc import Callable
from typing import Any

import pytest

from app_mcp import ToolCallError
from app_mcp.uifallback import (
    FillBool,
    FillNumber,
    FillText,
    UiDescription,
    UiElement,
    UiEntry,
    UiEntryKind,
    UiFallbackTools,
    UiInspector,
    UiKey,
    UiScrollDirection,
    UiWindow,
)
from app_mcp.uifallback import _format as fmt
from app_mcp.uifallback import _input as inp
from app_mcp.uifallback._tree import TRANSPARENT

# ---------------------------------------------------------------------------
# 格式（第 4、7 节）
# ---------------------------------------------------------------------------


def _window(ref: str) -> UiEntry:
    return UiEntry(UiEntryKind.CONTAINER, ref, "window", "窗口", ref, "商城")


def _item(ref: str, role: str, name: str, value: str | None = None, states: tuple[str, ...] = (), declared: str | None = None, depth: int = 1, containers: tuple[str, ...] = ("e1",)) -> UiEntry:  # fmt: skip
    return UiEntry(UiEntryKind.ITEM, ref, role, fmt.label(role), ref, name, value, states, declared=declared,
                   depth=depth, chain=(0,), containers=containers)  # fmt: skip


def test_render_lines_items_and_limits() -> None:
    entries = [
        _window("e1"),
        _item("e2", "button", "清空", declared="cart.clear"),
        _item("e3", "button", "结算", states=("disabled",)),
        _item("e4", "textbox", "备注", value="尽快"),
    ]
    o = fmt.render(entries, None, 60)
    assert o.text == '» e1 窗口「商城」\n  e2 按钮「清空」 [已声明：cart.clear]\n  e3 按钮「结算」 disabled\n  e4 输入框「备注」= "尽快"'
    j = o.to_json()
    assert j["total"] == 3 and "remaining" not in j
    assert j["items"][0] == {"ref": "e2", "role": "button", "name": "清空", "declared": "cart.clear", "group": "窗口「商城」"}
    assert j["hint"].startswith("标注 [已声明：…]")
    limited = fmt.render(entries, None, 1)
    assert limited.remaining == 2
    assert limited.text.endswith("…另有 2 个元素未列出，可用 query 或 within 缩小范围")
    assert fmt.render(entries, "付款", 60).text == "（没有与「付款」匹配的可交互元素）"
    assert fmt.render([], None, 60).text == "（没有可见的可交互元素）"
    assert [e.ref for e in fmt.render(entries, "商城 结算", 60).items] == ["e3"]


def test_diff_reports_changes_added_groups_and_removals() -> None:
    before = [_window("e1"), _item("e2", "checkbox", "同意", states=("unchecked",)), _item("e3", "button", "删除")]
    dialog = UiEntry(UiEntryKind.CONTAINER, "e9", "dialog", "对话框", "e9", "确认", depth=1, chain=(0,), containers=("e1",))
    after = [
        _window("e1"),
        _item("e2", "checkbox", "同意", states=("checked", "focused")),
        dialog,
        _item("e10", "button", "确定", depth=2, containers=("e1", "e9")),
        _item("e11", "button", "取消", depth=2, containers=("e1", "e9")),
    ]
    assert fmt.diff(before, after, "ui") == [
        "e2 同意 变为 checked",
        '新增对话框「确认」(e9)，含 2 个可交互元素（可用 ui.outline({ within: "e9" }) 查看）',
        "按钮「删除」(e3) 已消失",
    ]
    many = [_item(f"e{i + 1}", "button", f"b{i}") for i in range(1, 21)]
    changes = fmt.diff([_window("e1")], [_window("e1"), *many], "ui")
    assert len(changes) == 16
    assert changes[-1] == "…另有 5 项变化，请调用 ui.outline 查看"
    value_changed = fmt.diff([_item("e2", "textbox", "备注", value="a")], [_item("e2", "textbox", "备注")], "ui")
    assert value_changed == ["e2 备注 值已清空"]


def test_truncate_collapse_and_read_text() -> None:
    assert fmt.truncate("abc", 3) == "abc"
    assert fmt.truncate("abcd", 3) == "ab…"
    assert fmt.truncate("😀😀😀😀", 3) == "😀😀…"
    assert fmt.collapse("  a \n b ") == "a b"
    assert fmt.REF_PATTERN.match("e12") and not fmt.REF_PATTERN.match("e0")
    r = fmt.read_text(["确定", " 确定 ", "取消"], None, 100)
    assert (r.ref, r.text, r.truncated) == ("root", "确定 取消", False)
    r = fmt.read_text(["abcdef"], "e3", 4)
    assert (r.ref, r.text, r.truncated) == ("e3", "abc…", True)


def _kind(fn: Callable[[], Any]) -> tuple[str, Any]:
    with pytest.raises(ToolCallError) as e:
        fn()
    return e.value.kind, e.value.details


def test_parse_arguments() -> None:
    args = {"ref": " e3 ", "limit": 2.7, "query": "a", "value": True}
    assert inp.arg_ref(args, "ref") == "e3"
    assert inp.arg_int(args, "limit") == 2
    assert inp.arg_value(args) == FillBool(True)
    assert inp.arg_value({"value": 3}) == FillNumber(3.0)
    assert inp.arg_value({"value": "3"}) == FillText("3")
    assert inp.arg_ref({"within": None}, "within", required=False) is None
    assert inp.as_text(FillNumber(3.0), "x", None) == "3"
    assert inp.as_text(FillNumber(2.5), "x", None) == "2.5"
    assert inp.as_number(FillText(" 4 "), "x", None) == 4.0
    assert inp.parse_key("shift+TAB") is UiKey.SHIFT_TAB
    assert inp.parse_key(" ") is UiKey.SPACE
    assert inp.parse_direction("left") is UiScrollDirection.LEFT
    bad: list[Callable[[], Any]] = [
        lambda: inp.arg_ref({"ref": "12"}, "ref"),
        lambda: inp.arg_ref({}, "ref"),
        lambda: inp.arg_int({"limit": "2"}, "limit"),
        lambda: inp.arg_int({"limit": True}, "limit"),
        lambda: inp.arg_string({"query": 1}, "query"),
        lambda: inp.arg_value({}),
        lambda: inp.arg_value({"value": None}),
        lambda: inp.parse_key("F5"),
        lambda: inp.parse_direction("forward"),
        lambda: inp.as_bool(FillText("yes"), "x", "e1"),
        lambda: inp.as_number(FillText("abc"), "x", "e1"),
    ]
    for fn in bad:
        assert _kind(fn)[0] == "INVALID_INPUT"
    assert "支持 Enter、Escape、Tab、Shift+Tab、Space" in str(pytest.raises(ToolCallError, inp.parse_key, "F5").value)


def test_schemas_reject_additional_properties() -> None:
    schemas = inp.schemas(60)
    assert set(schemas) == {"outline", "click", "fill", "press", "scroll", "read"}
    assert all(s["additionalProperties"] is False for s in schemas.values())
    assert schemas["outline"]["properties"]["limit"]["maximum"] == fmt.LIMIT_MAX
    assert schemas["read"]["properties"]["maxChars"]["maximum"] == fmt.READ_MAX
    assert schemas["scroll"]["properties"]["direction"]["enum"] == ["up", "down", "left", "right"]


# ---------------------------------------------------------------------------
# 假界面
# ---------------------------------------------------------------------------


class Node(UiElement):
    def __init__(self, role: str | None, name: str = "", *children: Node, **kw: Any) -> None:
        self.role, self.name, self.kids = role, name, list(children)
        self.value: str | None = kw.get("value")
        self.states: list[str] = list(kw.get("states", ()))
        self.hidden = kw.get("hidden", False)
        self.enabled = kw.get("enabled", True)
        self.secure = kw.get("secure", False)
        self.declared: str | None = kw.get("declared")
        self.container = kw.get("container", False)
        self.options: list[str] = kw.get("options", [])
        self.on_click: Callable[[], bool] | None = kw.get("on_click")
        self.scroll_ok: bool = kw.get("scroll_ok", False)
        self.reuse = kw.get("reusable", False)
        self.log: list[Any] = []

    @property
    def identity(self) -> Any:
        return self

    @property
    def reusable(self) -> bool:
        return self.reuse

    def is_hidden(self) -> bool:
        return self.hidden

    def describe(self) -> UiDescription:
        if self.role is None:
            return UiDescription(None, declared=self.declared) if self.declared else TRANSPARENT
        kind = UiEntryKind.CONTAINER if self.container else UiEntryKind.ITEM
        return UiDescription(
            kind, self.role, self.name, value=self.value, states=tuple(self.states), secure=self.secure,
            has_secure_value=bool(self.value), declared=self.declared,
        )  # fmt: skip

    def children(self) -> list[UiElement]:
        return list(self.kids)

    def read_text(self) -> list[str]:
        return [self.name, fmt.SECURE_MASK if self.secure and self.value else (self.value or "")]

    def is_enabled(self) -> bool:
        return self.enabled

    def click(self) -> bool:
        self.log.append("click")
        if self.on_click is not None:
            return self.on_click()
        if self.role in ("checkbox", "radio", "switch"):
            on = "checked" in self.states
            self.states = [s for s in self.states if s not in ("checked", "unchecked")] + ["unchecked" if on else "checked"]
            return True
        return self.role == "button"

    def set_text(self, text: str) -> bool:
        self.value = text
        return True

    def select_option(self, text: str) -> bool:
        if text not in self.options:
            raise inp.invalid(f"下拉框没有选项「{text}」；可选：{'、'.join(self.options)}")
        self.value = text
        return True

    def set_range(self, value: float) -> bool:
        self.value = str(int(value))
        return True

    def focus(self) -> bool:
        self.log.append("focus")
        return True

    def ime_action(self) -> bool:
        self.log.append("ime")
        return True

    def scroll_into_view(self) -> bool:
        self.log.append("into-view")
        if self.scroll_ok:
            self.hidden = False
        return self.scroll_ok

    def scroll_page(self, direction: UiScrollDirection) -> bool:
        self.log.append(direction.value)
        return self.scroll_ok


class Win(Node, UiWindow):
    def __init__(self, name: str, *children: Node, role: str = "window") -> None:
        super().__init__(role, name, *children, container=True)
        self.keys: list[UiKey] = []
        self.handles: set[UiKey] = set()
        self.on_dismiss: Callable[[], bool] = lambda: False
        self.focus_moves: list[bool] = []

    def send_key(self, key: UiKey) -> bool:
        self.keys.append(key)
        return key in self.handles

    def move_focus(self, forward: bool) -> bool:
        self.focus_moves.append(forward)
        return True

    def dismiss(self) -> bool:
        return self.on_dismiss()


class FakePlatform:
    def __init__(self, *windows: Win) -> None:
        self.wins = list(windows)
        self.modal: Win | None = None
        self.performed = 0

    def windows(self) -> list[UiWindow]:
        return [self.modal] if self.modal is not None else list(self.wins)

    def all_windows(self) -> list[UiWindow]:
        return [*self.wins, *([self.modal] if self.modal is not None else [])]

    def is_live(self, identity: Any) -> bool:
        def attached(e: UiElement) -> bool:
            return e is identity or any(attached(c) for c in e.children())

        return any(attached(w) for w in self.all_windows())

    async def run(self, fn: Callable[[], Any]) -> Any:
        return fn()

    async def perform(self, fn: Callable[[], bool]) -> bool:
        self.performed += 1
        return fn()

    async def settle(self) -> None:
        return None


def run(coro: Any) -> Any:
    return asyncio.run(coro)


def error(coro: Any) -> ToolCallError:
    with pytest.raises(ToolCallError) as e:
        run(coro)
    return e.value


class Shop:
    """一个商城窗口：按钮、复选框、单选、文本框、密码框、下拉框、滑块、滚动区。"""

    def __init__(self) -> None:
        self.pay = Node("button", "结算", states=["disabled"], enabled=False)
        self.clear = Node("button", "清空", declared="cart.clear")
        self.agree = Node("checkbox", "同意", states=["unchecked"])
        self.fast = Node("radio", "快递", states=["checked"])
        self.note = Node("textbox", "备注", value="尽快")
        self.readonly = Node("textbox", "单号", value="123", states=["readonly"])
        self.password = Node("textbox", "支付密码", value="secret", secure=True)
        self.city = Node("combobox", "城市", value="北京", options=["北京", "上海"])
        self.volume = Node("slider", "音量", value="1")
        self.far = Node("button", "底部", hidden=True, scroll_ok=True)
        self.wrapped = Node("button", "外层声明")
        self.label = Node("generic", "静态")
        self.label.click = lambda: False  # type: ignore[method-assign]
        self.area = Node("scrollable", "列表", self.far, container=True, scroll_ok=True)
        self.window = Win(
            "商城",
            self.pay, self.clear, self.agree, self.fast, self.note, self.readonly, self.password, self.city,
            self.volume, self.area, Node(None, "", self.wrapped, declared="cart.wrapped"), self.label,
        )  # fmt: skip
        self.platform = FakePlatform(self.window)
        self.ui = UiInspector(self.platform)

    def ref(self, name: str) -> str:
        outline = run(self.ui.outline(name, None, None))
        refs = [i.ref for i in outline.items if i.name == name]
        assert len(refs) == 1, outline.text
        return refs[0]  # type: ignore[return-value]


def test_outline_groups_marks_declared_and_masks_secure() -> None:
    s = Shop()
    o = run(s.ui.outline(None, None, None))
    lines = o.text.split("\n")
    assert lines[0] == "» e1 窗口「商城」"
    assert "  e3 按钮「清空」 [已声明：cart.clear]" in lines
    assert '  e8 密码框「支付密码」= "••••"' in lines
    assert "  e2 按钮「结算」 disabled" in lines
    assert any("外层声明」 [已声明：cart.wrapped]" in line for line in lines)
    assert not any("滚动区" in line for line in lines)  # 滚动区内没有可见控件：分组不输出
    assert "secret" not in o.text
    assert "底部" not in o.text  # 屏外
    # 引用稳定：再次调用得到同一引用
    assert run(s.ui.outline(None, None, None)).text == o.text
    # within：只列出分组子树
    s.far.hidden = False
    items = run(s.ui.outline("底部", None, None)).to_json()["items"]
    assert items[0]["group"] == "窗口「商城」 › 滚动区「列表」"
    text = run(s.ui.outline(None, None, None)).text
    area = next(line.split()[1] for line in text.split("\n") if "滚动区「列表」" in line)
    sub = run(s.ui.outline(None, "e1", 2))
    assert sub.total > 2 and sub.remaining == sub.total - 2
    scoped = run(s.ui.outline(None, area, None))
    assert [i.name for i in scoped.items] == ["底部"], scoped.text


def test_outline_limit_is_clamped() -> None:
    s = Shop()
    assert len(run(s.ui.outline(None, None, 0)).items) == 1
    many = Win("大", *[Node("button", f"b{i}") for i in range(600)])
    ui = UiInspector(FakePlatform(many), max_items=60)
    assert len(run(ui.outline(None, None, None)).items) == 60
    o = run(ui.outline(None, None, 10_000))
    assert len(o.items) == fmt.LIMIT_MAX and o.remaining == 100


def test_click_reports_changes_and_declared_hint() -> None:
    s = Shop()

    def enable_pay() -> bool:
        s.pay.enabled, s.pay.states = True, []
        s.window.kids.append(Win("确认", Node("button", "确定"), Node("button", "取消"), role="dialog"))
        return True

    s.clear.on_click = enable_pay
    result = run(s.ui.click(s.ref("清空")))
    assert result.changes[0] == "e2 结算 不再 disabled"
    assert result.changes[1].startswith("新增对话框「确认」(") and "含 2 个可交互元素" in result.changes[1]
    assert result.hint == "该元素已声明为工具 cart.clear，下次可直接调用"
    checkbox = run(s.ui.click(s.ref("同意")))
    assert checkbox.to_json() == {"ok": True, "changes": [f"{s.ref('同意')} 同意 变为 checked"]}


def test_precondition_errors() -> None:
    s = Shop()
    pay, note, pw, label = s.ref("结算"), s.ref("备注"), s.ref("支付密码"), s.ref("静态")
    e = error(s.ui.click(pay))
    assert (e.kind, e.details) == ("INVALID_INPUT", {"ref": pay, "reason": "TOOL_DISABLED"})
    assert e.message == f"{pay} 按钮「结算」已禁用，当前无法操作"
    e = error(s.ui.fill(pw, FillText("x")))
    assert e.details == {"ref": pw, "reason": "secure"} and pw in e.message
    assert s.password.value == "secret"
    e = error(s.ui.press(pw, "Enter"))
    assert e.details == {"ref": pw, "reason": "secure"}
    e = error(s.ui.click(label))
    assert e.details == {"ref": label, "reason": "unsupported"}
    # 已隐藏（仍在界面上）→ hidden；已移除 → 失效
    s.note.hidden = True
    e = error(s.ui.click(note))
    assert e.details == {"ref": note, "reason": "hidden"}
    s.window.kids.remove(s.note)
    e = error(s.ui.click(note))
    assert (e.details, e.message) == ({"ref": note}, f"引用 {note} 已失效，请重新调用 ui.outline")
    assert error(s.ui.click("e999")).details == {"ref": "e999"}


def test_modal_occludes_lower_windows() -> None:
    s = Shop()
    note = s.ref("备注")
    ok = Node("button", "确定")
    s.platform.modal = Win("确认", ok, role="dialog")
    o = run(s.ui.outline(None, None, None))
    assert "备注" not in o.text and "确定" in o.text
    e = error(s.ui.fill(note, FillText("x")))
    assert e.details == {"ref": note, "reason": "hidden"}
    assert s.note.value == "尽快"
    s.platform.modal = None
    assert run(s.ui.fill(note, FillText("x"))).changes == (f'{note} 备注 值变为 "x"',)


def test_fill_by_role() -> None:
    s = Shop()
    agree, fast, city, volume, readonly = s.ref("同意"), s.ref("快递"), s.ref("城市"), s.ref("音量"), s.ref("单号")
    assert run(s.ui.fill(agree, FillBool(True))).changes == (f"{agree} 同意 变为 checked",)
    assert run(s.ui.fill(agree, FillBool(True))).changes == ()  # 已是目标状态：不再点击
    assert s.agree.log == ["click"]
    assert error(s.ui.fill(fast, FillBool(False))).details == {"ref": fast, "reason": "unsupported"}
    assert error(s.ui.fill(agree, FillText("yes"))).details == {"ref": agree}
    assert run(s.ui.fill(city, FillText("上海"))).changes == (f'{city} 城市 值变为 "上海"',)
    e = error(s.ui.fill(city, FillText("广州")))
    assert "可选：北京、上海" in e.message
    assert run(s.ui.fill(volume, FillNumber(7))).changes == (f'{volume} 音量 值变为 "7"',)
    assert error(s.ui.fill(readonly, FillText("x"))).details == {"ref": readonly, "reason": "unsupported"}
    assert error(s.ui.fill(s.ref("清空"), FillText("x"))).details["reason"] == "unsupported"


def test_press_keys() -> None:
    s = Shop()
    note = s.ref("备注")
    run(s.ui.press(note, "Enter"))
    assert s.note.log == ["focus", "ime"]
    run(s.ui.press(None, "Tab"))
    run(s.ui.press(None, "Shift+Tab"))
    assert s.window.focus_moves == [True, False]
    # Escape：顶层未处理时 dismiss；都不处理时给提示
    assert run(s.ui.press(None, "Escape")).hint == "按键 Escape 没有被任何控件处理"
    s.window.on_dismiss = lambda: True
    assert run(s.ui.press(None, "Escape")).hint is None
    # Space：窗口未处理 → 激活目标控件
    agree = s.ref("同意")
    run(s.ui.press(agree, "Space"))
    assert s.agree.log == ["focus", "click"] and "checked" in s.agree.states
    s.window.handles.add(UiKey.SPACE)
    run(s.ui.press(agree, " "))
    assert s.agree.log == ["focus", "click", "focus"]
    # 当前焦点是密码框 → 拒绝
    s.password.states.append("focused")
    assert error(s.ui.press(None, "Enter")).details == {"reason": "secure"}
    assert error(s.ui.press(None, "F1")).kind == "INVALID_INPUT"


def test_scroll() -> None:
    s = Shop()
    s.far.hidden = False
    far = s.ref("底部")
    s.far.hidden = True
    # 屏外控件可以滚动到可见（只核对仍在界面上）
    result = run(s.ui.scroll(far, None))
    assert s.far.log == ["into-view"] and any(far in c for c in result.changes)
    assert run(s.ui.scroll(far, None)).changes == ()  # 已可见：不动
    s.far.scroll_ok = False
    assert run(s.ui.scroll(far, "down")).hint == "已到尽头或不可滚动"
    assert s.far.log == ["into-view", "down"]
    s.far.scroll_ok = True
    assert run(s.ui.scroll(far, "up")).hint is None
    s.far.hidden, s.far.scroll_ok = True, False
    assert run(s.ui.scroll(far, None)).hint == "没有可滚动的祖先，无法滚动到可见"
    s.area.kids.clear()
    assert error(s.ui.scroll(far, None)).details == {"ref": far}


def test_read_masks_secure_and_truncates() -> None:
    s = Shop()
    whole = run(s.ui.read(None, None))
    assert "商城" in whole.text and "••••" in whole.text and "secret" not in whole.text and "底部" not in whole.text
    note = s.ref("备注")
    assert run(s.ui.read(note, None)).to_json() == {"ref": note, "text": "备注 尽快", "truncated": False}
    r = run(s.ui.read(note, 3))
    assert (r.text, r.truncated) == ("备注…", True)
    long = Win("长", Node("generic", "字" * 30000))
    r = run(UiInspector(FakePlatform(long)).read(None, 10**9))
    assert len(r.text) == fmt.READ_MAX and r.truncated
    r = run(UiInspector(FakePlatform(long)).read(None, None))
    assert len(r.text) == fmt.READ_DEFAULT


def test_reusable_elements_are_verified_by_fingerprint() -> None:
    row = Node("option", "苹果", reusable=True)
    window = Win("列表", row)
    ui = UiInspector(FakePlatform(window))
    ref = run(ui.outline(None, None, None)).items[0].ref
    row.name = "香蕉"  # 同一行对象复用给了另一个数据项
    items = run(ui.outline(None, None, None)).items
    assert items[0].ref != ref
    assert error(ui.click(ref)).details == {"ref": ref}
    # 不复用的控件改名后引用不变
    button = Node("button", "确定")
    ui2 = UiInspector(FakePlatform(Win("w", button)))
    before = run(ui2.outline(None, None, None)).items[0].ref
    button.name = "好"
    assert run(ui2.outline(None, None, None)).items[0].ref == before
    # 重建的控件按唯一指纹沿用原引用
    window.kids = [Node("option", "香蕉", reusable=True)]
    assert run(ui.outline(None, None, None)).items[0].ref == items[0].ref


def test_clear_drops_all_refs() -> None:
    s = Shop()
    note = s.ref("备注")
    s.ui.clear()
    assert error(s.ui.click(note)).details == {"ref": note}


# ---------------------------------------------------------------------------
# 工具注册（第 2 节）
# ---------------------------------------------------------------------------


class FakeHandle:
    def __init__(self, name: str, fn: Callable[..., Any], kw: dict[str, Any]) -> None:
        self.name, self.fn, self.kw = name, fn, kw
        self.enabled = kw["enabled"]

    def set_enabled(self, enabled: bool) -> None:
        self.enabled = enabled


class FakeScope:
    def __init__(self, name: str) -> None:
        self.name = name
        self.tools: dict[str, FakeHandle] = {}
        self.disposed = False

    def add_tool(self, fn: Callable[..., Any], name: str, description: str, **kw: Any) -> FakeHandle:
        handle = FakeHandle(name, fn, {"description": description, **kw})
        self.tools[name] = handle
        return handle

    def dispose(self) -> None:
        self.disposed = True


class FakeRegistrar:
    def __init__(self) -> None:
        self.scopes: list[FakeScope] = []

    def scope(self, name: str) -> FakeScope:
        self.scopes.append(FakeScope(name))
        return self.scopes[-1]


def test_tools_register_view_surface_and_follow_visibility() -> None:
    s = Shop()
    registrar = FakeRegistrar()
    tools = UiFallbackTools(registrar, s.ui)
    scope = registrar.scopes[0]
    assert scope.name == "ui-fallback"
    assert sorted(scope.tools) == ["ui.click", "ui.fill", "ui.outline", "ui.press", "ui.read", "ui.scroll"]
    for name, h in scope.tools.items():
        read_only = name in ("ui.outline", "ui.read")
        assert h.kw["surface"] == "view" and h.kw["enabled"] is False
        assert h.kw["annotations"]["read_only_hint"] is read_only
        assert h.kw["risk"] == ("read" if read_only else "write")
        assert h.kw["input_schema"]["additionalProperties"] is False
    outline = scope.tools["ui.outline"].fn
    e = error(outline())
    assert e.kind == "TOOL_DISABLED"
    assert tools.set_visible(True) and not tools.set_visible(True)
    assert all(h.enabled for h in scope.tools.values())
    assert run(outline(limit=1))["remaining"] > 0
    note = s.ref("备注")
    assert run(scope.tools["ui.fill"].fn(ref=note, value="好"))["changes"] == [f'{note} 备注 值变为 "好"']
    assert error(scope.tools["ui.click"].fn()).kind == "INVALID_INPUT"
    assert error(scope.tools["ui.press"].fn(ref=note)).message == "缺少参数 key"
    assert run(scope.tools["ui.read"].fn(ref=note, maxChars=2))["truncated"] is True
    assert run(scope.tools["ui.scroll"].fn(ref=note))["ok"] is True
    tools.set_visible(False)
    assert not any(h.enabled for h in scope.tools.values())
    tools.close()
    tools.close()
    assert scope.disposed and not tools.set_visible(True)
