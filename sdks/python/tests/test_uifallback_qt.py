"""Qt Widgets 控件兜底（spec/ui-fallback.md）：真实 PySide6 / PyQt6 控件，offscreen 平台。

没有安装 Qt 时跳过。引擎在后台 asyncio 线程上运行，主线程运行 Qt 事件循环（与真实 App 一致）；
模态对话框（``exec()``）期间主线程停在嵌套事件循环里，场景协程照样经排队信号访问控件。
"""

from __future__ import annotations

import asyncio
import os
import threading
from collections.abc import Callable, Coroutine
from typing import Any

import pytest

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

try:
    from PySide6 import QtCore, QtWidgets  # type: ignore
except ImportError:  # pragma: no cover - 取决于环境
    try:
        from PyQt6 import QtCore, QtWidgets  # type: ignore
    except ImportError:
        pytest.skip("需要 PySide6 或 PyQt6", allow_module_level=True)

from app_mcp import AppMcp, ToolCallError
from app_mcp.uifallback import FillBool, FillNumber, FillText
from app_mcp.uifallback.qt import QtUiFallback, QtUiFallbackOptions, declare_mcp_tools, undeclare_mcp_tools

Qt = QtCore.Qt
W = QtWidgets

APP = W.QApplication.instance() or W.QApplication([])
LOOP = asyncio.new_event_loop()
threading.Thread(target=LOOP.run_forever, name="ui-fallback-test-loop", daemon=True).start()


def _close_modals() -> None:
    """超时时关闭模态对话框 / 弹出层，让主线程离开嵌套事件循环（测试以超时失败而不是挂起）。"""
    for _ in range(10):
        w = W.QApplication.activeModalWidget() or W.QApplication.activePopupWidget()
        if w is None:
            return
        w.close()


def drive(coro: Coroutine[Any, Any, Any], timeout: float = 30) -> Any:
    """在后台循环上运行场景协程，主线程同时运行 Qt 事件循环直到场景结束。"""
    future = asyncio.run_coroutine_threadsafe(coro, LOOP)
    loop = QtCore.QEventLoop()
    poll = QtCore.QTimer()
    poll.setInterval(5)
    poll.timeout.connect(lambda: loop.quit() if future.done() else None)
    poll.start()
    QtCore.QTimer.singleShot(int(timeout * 1000), lambda: (_close_modals(), loop.quit()))
    loop.exec()
    poll.stop()
    return future.result(timeout=0)


class ShopWindow:
    """被测界面。"""

    def __init__(self) -> None:
        self.log: list[str] = []
        self.window = W.QMainWindow()
        self.window.setWindowTitle("商城")
        menu = self.window.menuBar().addMenu("文件(&F)")
        self.export = menu.addAction("导出")
        self.export.triggered.connect(lambda: self.log.append("export"))
        central = W.QWidget()
        root = W.QVBoxLayout(central)
        form = W.QFormLayout()
        self.name = W.QLineEdit()
        self.name.textEdited.connect(lambda t: self.log.append(f"edited:{t}"))
        form.addRow("姓名(&N)", self.name)
        self.password = W.QLineEdit("secret")
        self.password.setEchoMode(W.QLineEdit.EchoMode.Password)
        form.addRow("密码", self.password)
        self.qty = W.QSpinBox()
        self.qty.setRange(0, 10)
        form.addRow("数量", self.qty)
        self.city = W.QComboBox()
        self.city.addItems(["北京", "上海"])
        self.city.activated.connect(lambda i: self.log.append(f"activated:{i}"))
        form.addRow("城市", self.city)
        self.volume = W.QSlider(Qt.Orientation.Horizontal)
        self.volume.setAccessibleName("音量")
        self.volume.setRange(0, 100)
        form.addRow(self.volume)
        self.submit = W.QLineEdit()
        self.submit.setAccessibleName("提交框")
        self.submit.returnPressed.connect(lambda: self.log.append("return"))
        form.addRow(self.submit)
        root.addLayout(form)
        self.agree = W.QCheckBox("同意")
        self.express = W.QRadioButton("快递")
        self.express.setChecked(True)
        self.pickup = W.QRadioButton("自提")
        for w in (self.agree, self.express, self.pickup):
            root.addWidget(w)
        self.clear = W.QPushButton("清空(&C)")
        self.pay = W.QPushButton("结算")
        self.pay.setEnabled(False)
        self.clear.clicked.connect(lambda: self.pay.setEnabled(True))
        self.confirm = W.QPushButton("确认")
        self.confirm.clicked.connect(self._open_dialog)
        for w in (self.clear, self.pay, self.confirm):
            root.addWidget(w)
        self.tabs = W.QTabWidget()
        self.tabs.addTab(W.QPushButton("基本按钮"), "基本")
        self.tabs.addTab(W.QPushButton("高级按钮"), "高级")
        root.addWidget(self.tabs)
        self.fruits = W.QListWidget()
        self.fruits.addItems([f"水果{i}" for i in range(40)])
        self.fruits.setFixedHeight(80)
        self.fruits.clicked.connect(lambda idx: self.log.append(f"row:{idx.row()}"))
        root.addWidget(self.fruits)
        self.area = W.QScrollArea()
        self.area.setAccessibleName("长内容")
        content = W.QWidget()
        content.setFixedSize(300, 900)
        self.top = W.QPushButton("顶部", content)
        self.top.move(10, 10)
        self.bottom = W.QPushButton("底部", content)
        self.bottom.move(10, 850)
        self.area.setWidget(content)
        self.area.setFixedHeight(120)
        root.addWidget(self.area)
        self.window.setCentralWidget(central)
        self.window.resize(480, 1000)
        self.dialog_result: int | None = None

    def _open_dialog(self) -> None:
        dialog = W.QDialog(self.window)
        dialog.setWindowTitle("确认支付")
        layout = W.QVBoxLayout(dialog)
        ok = W.QPushButton("确定")
        ok.clicked.connect(dialog.accept)
        layout.addWidget(ok)
        layout.addWidget(W.QLineEdit())
        self.dialog_result = dialog.exec()
        dialog.deleteLater()


@pytest.fixture
def shop() -> Any:
    s = ShopWindow()
    s.window.show()
    APP.processEvents()
    client = AppMcp(app_id="uifallback-qt-test", app_name="Qt 兜底测试", host_url="ws://127.0.0.1:9")
    fallback = QtUiFallback.enable(client, QtUiFallbackOptions(settle_delay=0.02))
    s.fallback = fallback  # type: ignore[attr-defined]
    s.ui = fallback.inspector  # type: ignore[attr-defined]
    yield s
    fallback.close()
    client.close()
    s.window.close()
    s.window.deleteLater()
    APP.processEvents()


async def gui(s: Any, fn: Callable[[], Any]) -> Any:
    return await s.ui.platform.run(fn)


async def ref(s: Any, name: str, query: str | None = None) -> str:
    outline = await s.ui.outline(query or name, None, None)
    found = [i.ref for i in outline.items if i.name == name]
    assert len(found) == 1, outline.text
    return found[0]


async def failure(coro: Coroutine[Any, Any, Any]) -> ToolCallError:
    try:
        await coro
    except ToolCallError as e:
        return e
    raise AssertionError("应当失败")


def test_outline_maps_widgets(shop: Any) -> None:
    declare_mcp_tools(shop.clear, "cart.clear")

    async def scenario() -> str:
        return (await shop.ui.outline(None, None, 200)).text

    text = drive(scenario())
    lines = [line.strip() for line in text.split("\n")]
    assert lines[0] == "» e1 窗口「商城」"
    expected = [
        "导航「菜单栏」", "菜单项「文件(F)」 collapsed", "输入框「姓名(N)」", '密码框「密码」= "••••"', '数字框「数量」= "0"',
        '下拉框「城市」= "北京" collapsed', '滑块「音量」= "0"', "复选框「同意」 unchecked", "单选框「快递」 checked",
        "按钮「清空(C)」 [已声明：cart.clear]", "按钮「结算」 disabled", "标签页「基本」 selected", "标签页「高级」",
        "按钮「基本按钮」", "列表", "选项「水果0」", "滚动区「长内容」", "按钮「顶部」",
    ]  # fmt: skip
    for part in expected:
        assert any(part in line for line in lines), (part, text)
    assert "secret" not in text
    assert "底部" not in text and "高级按钮" not in text and "水果39" not in text  # 屏外 / 未选中的标签页 / 滚出视口
    undeclare_mcp_tools(shop.clear, "cart.clear")
    assert "已声明" not in drive(scenario())


def test_fill_click_and_read(shop: Any) -> None:
    async def scenario() -> None:
        s = shop
        name = await ref(s, "姓名(N)")
        r = await s.ui.fill(name, FillText("张三"))
        assert r.changes == (f'{name} 姓名(N) 值变为 "张三"',)
        assert await gui(s, s.name.text) == "张三"
        pw = await ref(s, "密码")
        e = await failure(s.ui.fill(pw, FillText("x")))
        assert e.details == {"ref": pw, "reason": "secure"}
        assert await gui(s, s.password.text) == "secret"
        await s.ui.fill(await ref(s, "数量"), FillNumber(5))
        assert await gui(s, s.qty.value) == 5
        e = await failure(s.ui.fill(await ref(s, "数量"), FillNumber(50)))
        assert "0–10" in e.message
        city = await ref(s, "城市")
        await s.ui.fill(city, FillText("上海"))
        assert await gui(s, s.city.currentText) == "上海"
        assert "可选：北京、上海" in (await failure(s.ui.fill(city, FillText("广州")))).message
        await s.ui.fill(await ref(s, "音量"), FillNumber(30))
        assert await gui(s, s.volume.value) == 30
        agree = await ref(s, "同意")
        assert (await s.ui.fill(agree, FillBool(True))).changes == (f"{agree} 同意 变为 checked",)
        pickup = await ref(s, "自提")
        changes = (await s.ui.fill(pickup, FillBool(True))).changes
        assert f"{pickup} 自提 变为 checked" in changes and any("快递 变为 unchecked" in c for c in changes)
        pay = await ref(s, "结算")
        assert (await failure(s.ui.click(pay))).details == {"ref": pay, "reason": "TOOL_DISABLED"}
        clear = await ref(s, "清空(C)")
        assert (await s.ui.click(clear)).changes == (f"{pay} 结算 不再 disabled",)
        tab = await ref(s, "高级")
        changes = (await s.ui.click(tab)).changes
        assert f"{tab} 高级 变为 selected" in changes and any("新增按钮「高级按钮」" in c for c in changes)
        row = await ref(s, "水果1")
        assert f"{row} 水果1 变为 selected" in (await s.ui.click(row)).changes
        read = await s.ui.read(name, None)
        assert read.text == "张三"
        whole = await s.ui.read(None, None)
        assert "商城" in whole.text and "••••" in whole.text and "secret" not in whole.text

    drive(scenario())
    assert "edited:张三" in shop.log and "activated:1" in shop.log and "row:1" in shop.log


def test_menu_popup_occludes_window(shop: Any) -> None:
    async def scenario() -> None:
        s = shop
        name = await ref(s, "姓名(N)")
        menu = await ref(s, "文件(F)")
        changes = (await s.ui.click(menu)).changes
        assert any(c.startswith("新增列表「文件(F)」") for c in changes), changes
        outline = await s.ui.outline(None, None, None)
        assert [i.name for i in outline.items] == ["导出"], outline.text
        assert (await failure(s.ui.fill(name, FillText("x")))).details == {"ref": name, "reason": "hidden"}
        await s.ui.click(await ref(s, "导出"))
        assert await gui(s, lambda: W.QApplication.activePopupWidget() is None)
        await s.ui.click(menu)
        await s.ui.press(None, "Escape")
        assert await gui(s, lambda: W.QApplication.activePopupWidget() is None)

    drive(scenario())
    assert shop.log.count("export") == 1


def test_modal_dialog_exec_does_not_block(shop: Any) -> None:
    async def scenario() -> None:
        s = shop
        name = await ref(s, "姓名(N)")
        changes = (await s.ui.click(await ref(s, "确认"))).changes
        assert any(c.startswith("新增对话框「确认支付」") and "含 2 个可交互元素" in c for c in changes), changes
        assert any("已消失" in c for c in changes)  # 主窗口被模态对话框遮挡
        e = await failure(s.ui.click(name))
        assert e.details == {"ref": name, "reason": "hidden"}
        await s.ui.press(None, "Escape")
        assert await gui(s, lambda: s.dialog_result) == 0
        assert await ref(s, "姓名(N)") == name  # 对话框关闭后原引用恢复
        await s.ui.click(await ref(s, "确认"))
        await s.ui.press(await ref(s, "确定"), "Space")
        assert await gui(s, lambda: s.dialog_result) == 1

    drive(scenario())


def test_press_keys(shop: Any) -> None:
    async def scenario() -> None:
        s = shop
        submit = await ref(s, "提交框")
        await s.ui.press(submit, "Enter")
        await s.ui.press(await ref(s, "姓名(N)"), "Tab")
        assert await gui(s, lambda: s.window.focusWidget() is s.password)
        # 焦点在密码框上：缺省目标的按键一律拒绝
        assert (await failure(s.ui.press(None, "Shift+Tab"))).details == {"reason": "secure"}
        await s.ui.press(await ref(s, "数量"), "Shift+Tab")
        assert await gui(s, lambda: s.window.focusWidget() is s.password)
        await s.ui.press(await ref(s, "姓名(N)"), "Shift+Tab")
        assert await gui(s, lambda: s.window.focusWidget() is not s.name)
        e = await failure(s.ui.press(await ref(s, "密码"), "Enter"))
        assert e.details["reason"] == "secure"
        await s.ui.press(await ref(s, "同意"), "Space")
        assert await gui(s, s.agree.isChecked)
        assert (await s.ui.press(await ref(s, "姓名(N)"), "Escape")).hint == "按键 Escape 没有被任何控件处理"

    drive(scenario())
    assert shop.log.count("return") == 1


def test_scroll(shop: Any) -> None:
    async def scenario() -> None:
        s = shop
        top = await ref(s, "顶部")
        changes: list[str] = []
        for _ in range(10):
            r = await s.ui.scroll(top, "down")
            changes.extend(r.changes)
            if any("新增按钮「底部」" in c for c in changes):
                break
        assert any("新增按钮「底部」" in c for c in changes), changes
        assert any(f"按钮「顶部」({top}) 已消失" == c for c in changes)
        assert (await failure(s.ui.click(top))).details == {"ref": top, "reason": "hidden"}
        r = await s.ui.scroll(top, None)
        assert any(top in c for c in r.changes)
        assert (await s.ui.scroll(top, None)).changes == ()
        assert (await s.ui.scroll(top, "up")).hint == "已到尽头或不可滚动"
        first = await ref(s, "水果0")
        r = await s.ui.scroll(first, "down")
        assert any("水果0" in c and "已消失" in c for c in r.changes), r.changes
        assert (await s.ui.scroll(first, None)).changes != ()

    drive(scenario())


def test_tools_follow_window_visibility(shop: Any) -> None:
    fallback = shop.fallback
    assert fallback.enabled
    shop.window.hide()
    APP.processEvents()
    assert not fallback.enabled
    shop.window.show()
    APP.processEvents()
    assert fallback.enabled
    shop.window.showMinimized()
    APP.processEvents()
    fallback.refresh()
    assert not fallback.enabled
    shop.window.showNormal()
    APP.processEvents()
    fallback.refresh()
    assert fallback.enabled
    fallback.close()
    assert not fallback.enabled


def test_stale_after_widget_deleted(shop: Any) -> None:
    async def scenario() -> None:
        s = shop
        pickup = await ref(s, "自提")
        await gui(s, lambda: (s.pickup.setParent(None), s.pickup.deleteLater()))
        await asyncio.sleep(0.05)
        e = await failure(s.ui.click(pickup))
        assert e.details == {"ref": pickup}

    drive(scenario())
