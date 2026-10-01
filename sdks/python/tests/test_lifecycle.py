"""生命周期相关单元测试：不需要 Host（D-Bus 服务测试需要 jeepney 与会话总线，否则跳过）。"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import threading
import time
from pathlib import Path

import pytest

import app_mcp
from app_mcp import AppMcp, Hold, LifecyclePolicy, StateStatus, ToolCallError, WakeDescriptor
from app_mcp import app_mcp_uniffi as ffi
from app_mcp._client import _ClientListener, _complete_err, _lifecycle_to_ffi
from app_mcp.linux import (
    dbus_lifecycle,
    dbus_object_path,
    dbus_service_file,
    dbus_wake_descriptor,
    install_dbus_service,
    wake_arg_from_action,
)
from app_mcp.single_instance import SingleInstance, default_socket_path

SRC = Path(__file__).resolve().parents[1] / "src"
UNREACHABLE = "ws://127.0.0.1:9"


# ---------------------------------------------------------------------------
# 配置
# ---------------------------------------------------------------------------


def test_policy_defaults_and_conversion():
    p = LifecyclePolicy()
    assert (p.mode, p.residency, p.wake) == ("persistent", "keep", None)
    f = _lifecycle_to_ffi(p)
    assert f.mode == ffi.LifecycleMode.PERSISTENT
    assert (f.idle_timeout_ms, f.hidden_idle_timeout_ms, f.grace_ms) == (60000, 15000, 10000)

    p = LifecyclePolicy(
        mode="ON_DEMAND",
        idle_timeout=1.5,
        hidden_idle_timeout=0,
        grace=0.25,
        residency="exit_when_idle",
        wake=WakeDescriptor(kind="dbus", target="org.example.Shop", background=True),
    )
    assert p.mode == "on-demand" and p.residency == "exit-when-idle"
    f = _lifecycle_to_ffi(p)
    assert f.mode == ffi.LifecycleMode.ON_DEMAND
    assert f.residency == ffi.Residency.EXIT_WHEN_IDLE
    assert (f.idle_timeout_ms, f.hidden_idle_timeout_ms, f.grace_ms) == (1500, 0, 250)
    assert f.wake.kind == ffi.WakeKind.DBUS and f.wake.target == "org.example.Shop" and f.wake.background


def test_power_switches_conversion():
    p = LifecyclePolicy()
    assert (p.host_absent_retries, p.legacy_timers, p.merge_window, p.sleep_on_background) == (3, False, 2.0, False)
    f = _lifecycle_to_ffi(p)
    assert (f.host_absent_retries, f.legacy_timers, f.merge_window_ms, f.sleep_on_background) == (3, False, 2000, False)

    # 0 = 一直重连，与 uniffi 编码相同；合并窗口 0 = 调用后不额外停留
    f = _lifecycle_to_ffi(
        LifecyclePolicy(host_absent_retries=0, legacy_timers=True, merge_window=0.5, sleep_on_background=True)
    )
    assert (f.host_absent_retries, f.legacy_timers, f.merge_window_ms, f.sleep_on_background) == (0, True, 500, True)
    assert _lifecycle_to_ffi(LifecyclePolicy(merge_window=0)).merge_window_ms == 0


def test_heartbeat_config():
    from app_mcp._client import _HEARTBEATS, _enum_arg

    assert set(_HEARTBEATS.values()) == set(ffi.HeartbeatMode)
    for name, expected in (("auto", ffi.HeartbeatMode.AUTO), ("always", ffi.HeartbeatMode.ALWAYS), ("OFF", ffi.HeartbeatMode.OFF)):
        assert _enum_arg(name, _HEARTBEATS, "心跳策略") is expected
        AppMcp("py-hb", "Py", host_url=UNREACHABLE, heartbeat=name).close()
    AppMcp("py-hb", "Py", host_url=UNREACHABLE, heartbeat=ffi.HeartbeatMode.OFF).close()
    with pytest.raises(ValueError):
        AppMcp("py-hb", "Py", host_url=UNREACHABLE, heartbeat="sometimes")


def test_realtime_resource_changes_tools_hash():
    def tools_hash(**kwargs) -> str:
        client = AppMcp("py-rt", "Py", host_url=UNREACHABLE)
        try:
            client.add_resource(lambda: {}, "order", "订单", **kwargs)
            return client.tools_hash
        finally:
            client.close()

    plain = tools_hash()
    assert tools_hash(realtime=False) == plain
    assert tools_hash(realtime=True) != plain

    client = AppMcp("py-rt", "Py", host_url=UNREACHABLE)
    try:
        h0 = client.tools_hash

        @client.resource("live", description="实时", realtime=True)
        def live() -> dict:
            return {}

        assert "live" in client.resources and client.tools_hash != h0
    finally:
        client.close()


def test_dbus_lifecycle_is_desktop_default():
    p = dbus_lifecycle("org.example.Shop")
    assert p.mode == "idle"
    assert p.wake == dbus_wake_descriptor("org.example.Shop")
    assert (p.merge_window, p.sleep_on_background, p.host_absent_retries) == (2.0, False, 3)
    # 显式传入的字段总是生效
    custom = WakeDescriptor("uri", "shop", False)
    o = dbus_lifecycle("org.example.Shop", mode="on-demand", sleep_on_background=True, wake=custom, merge_window=0)
    assert (o.mode, o.sleep_on_background, o.wake, o.merge_window) == ("on-demand", True, custom, 0)
    with pytest.raises(ValueError):
        dbus_lifecycle("not a bus name")
    # AppMcp 本身默认不休眠
    client = AppMcp("py-def", "Py", host_url=UNREACHABLE)
    try:
        assert client.lifecycle is None
    finally:
        client.close()


@pytest.mark.parametrize(
    "kwargs",
    [
        {"mode": "sometimes"},
        {"residency": "forever"},
        {"idle_timeout": -1},
        {"merge_window": -1},
        {"host_absent_retries": -1},
    ],
)
def test_policy_validation(kwargs):
    with pytest.raises(ValueError):
        LifecyclePolicy(**kwargs)
    with pytest.raises(ValueError):
        WakeDescriptor(kind="carrier-pigeon")


def test_tool_call_error_details():
    e = ToolCallError("INVALID_INPUT", "坏参数", details={"field": "qty"})
    assert e.details == {"field": "qty"}
    assert ToolCallError("HANDLER_ERROR", "x").details is None


class _FakeCall:
    def __init__(self) -> None:
        self.calls: list[tuple] = []

    def fail(self, kind, message):
        self.calls.append(("fail", kind, message))

    def fail_with_details(self, kind, message, details_json):
        self.calls.append(("details", kind, message, json.loads(details_json)))


def test_complete_err_uses_details():
    c = _FakeCall()
    _complete_err(c, "INVALID_INPUT", "坏", {"field": "qty", "n": [1]})
    _complete_err(c, "HANDLER_ERROR", "无详情")
    _complete_err(c, "HANDLER_ERROR", "不可序列化", {"x": object()})
    assert c.calls[0] == ("details", "INVALID_INPUT", "坏", {"field": "qty", "n": [1]})
    assert c.calls[1] == ("fail", "HANDLER_ERROR", "无详情")
    assert c.calls[2] == ("fail", "HANDLER_ERROR", "不可序列化")  # 详情无法序列化：回退为不带详情

# ---------------------------------------------------------------------------
# 客户端生命周期 API（不连接 Host）
# ---------------------------------------------------------------------------


def test_client_lifecycle_api_without_host():
    exits: list[str] = []
    client = AppMcp(
        "py-life",
        "Py",
        host_url=UNREACHABLE,
        lifecycle=LifecyclePolicy(mode="on-demand"),
        connect_timeout=0.5,
        on_idle_exit=lambda: exits.append(threading.current_thread().name),
    )
    try:
        assert client.lifecycle.mode == "on-demand"
        assert len(client.tools_hash) == 16
        h0 = client.tools_hash

        @client.tool("x.ping", description="ping")
        def ping() -> str:
            return "pong"

        assert client.tools_hash != h0
        assert client.handle_wake("not a wake") is False
        assert client.handle_wake(["--foo", "bar"]) is False
        assert client.handle_wake([]) is False
        with pytest.raises(ValueError):
            client.wake("sideways")
        with pytest.raises(ValueError):
            client.sleep("nap")
        assert isinstance(client.sleep("background"), bool)

        with client.hold() as h:
            assert isinstance(h, Hold) and not h.released
        assert h.released
        h.release()  # 幂等
        h2 = client.hold()
        h2.release()
        assert h2.released

        # on_idle_exit：经 dispatcher（默认线程池）调用
        _ClientListener(client).on_idle_exit()
        deadline = time.monotonic() + 2
        while not exits and time.monotonic() < deadline:
            time.sleep(0.01)
        assert exits and exits[0].startswith("app-mcp")
    finally:
        client.close()


def test_on_demand_connect_now_leaves_dormant():
    client = AppMcp("py-od", "Py", host_url=UNREACHABLE, lifecycle=LifecyclePolicy(mode="on-demand"), connect_timeout=0.5)
    try:
        client.start()
        assert client.wait_for_state(StateStatus.DORMANT, timeout=2)
        assert client.connect_now() is True
        assert client.wait_for_state(StateStatus.BACKOFF, timeout=5)
    finally:
        client.close()


# ---------------------------------------------------------------------------
# Linux D-Bus 辅助
# ---------------------------------------------------------------------------


def test_dbus_templates(tmp_path: Path):
    assert dbus_object_path("org.example.my-shop") == "/org/example/my_shop"
    assert dbus_wake_descriptor("org.example.Shop") == WakeDescriptor("dbus", "org.example.Shop", True)
    text = dbus_service_file("org.example.Shop", ["/usr/bin/python3", "/opt/shop app/main.py"])
    assert text == "[D-BUS Service]\nName=org.example.Shop\nExec=/usr/bin/python3 '/opt/shop app/main.py'\n"
    assert dbus_service_file("org.example.Shop", "/usr/bin/shop").endswith("Exec=/usr/bin/shop\n")
    for bad in ("shop", "org..x", "1org.x"):
        with pytest.raises(ValueError):
            dbus_service_file(bad, "/bin/true")
    with pytest.raises(ValueError):
        dbus_service_file("org.example.Shop", "a\nb")
    path = install_dbus_service("org.example.Shop", "/usr/bin/shop", tmp_path / "svc")
    assert path.name == "org.example.Shop.service" and path.read_text().startswith("[D-BUS Service]")


def test_wake_arg_from_action():
    assert wake_arg_from_action("app-mcp-wake", ["tok"]) == "app-mcp-wake:tok"
    assert wake_arg_from_action("app-mcp-wake", [("s", "tok")]) == "app-mcp-wake:tok"
    assert wake_arg_from_action("app-mcp-wake", ["app-mcp-wake:tok"]) == "app-mcp-wake:tok"
    assert wake_arg_from_action("quit", ["tok"]) is None
    assert wake_arg_from_action("app-mcp-wake", []) is None
    assert wake_arg_from_action("app-mcp-wake", [("i", 3)]) is None
    assert ffi.parse_wake_token(wake_arg_from_action("app-mcp-wake", ["abc"])) == "abc"


class _Recorder:
    def __init__(self) -> None:
        self.args: list = []
        self.event = threading.Event()

    def handle_wake(self, argv) -> bool:
        self.args.append(argv)
        self.event.set()
        return True


def test_dbus_service_activate_action():
    pytest.importorskip("jeepney", reason="没有 jeepney")
    gdbus = shutil.which("gdbus")
    if gdbus is None or not os.environ.get("DBUS_SESSION_BUS_ADDRESS"):
        pytest.skip("没有 gdbus 或会话总线")
    from app_mcp.linux import serve_dbus_wake

    bus_name = f"dev.appmcp.PyTest{os.getpid()}"
    rec = _Recorder()
    try:
        service = serve_dbus_wake(rec, bus_name)
    except Exception as e:  # 会话总线不可用
        pytest.skip(f"无法连接会话总线：{e}")
    with service:
        r = subprocess.run(
            [gdbus, "call", "--session", "--dest", bus_name, "--object-path", dbus_object_path(bus_name),
             "--method", "org.freedesktop.Application.ActivateAction", "app-mcp-wake", "[<'tok42'>]", "{}"],
            capture_output=True, text=True, timeout=10,
        )
        assert r.returncode == 0, r.stderr
        assert rec.event.wait(5)
        assert rec.args == ["app-mcp-wake:tok42"]
        r = subprocess.run(
            [gdbus, "call", "--session", "--dest", bus_name, "--object-path", dbus_object_path(bus_name),
             "--method", "org.freedesktop.Application.Open", "['pyshop://app-mcp/wake?token=u1']", "", "{}"],
            capture_output=True, text=True, timeout=10,
        )
        assert r.returncode == 0, r.stderr
        assert rec.args[-1] == "pyshop://app-mcp/wake?token=u1"


# ---------------------------------------------------------------------------
# 单实例转发
# ---------------------------------------------------------------------------


def test_single_instance_forwarding(tmp_path: Path):
    sock = tmp_path / "run" / "shop.sock"
    received: list[list[str]] = []
    got = threading.Event()

    def on_argv(argv: list[str]) -> None:
        received.append(argv)
        got.set()

    primary = SingleInstance("shop", path=sock)
    assert primary.acquire(["--first"], on_argv) is True and primary.is_primary
    assert oct(sock.stat().st_mode & 0o777) == "0o600"
    assert oct(sock.parent.stat().st_mode & 0o777) == "0o700"
    try:
        second = SingleInstance("shop", path=sock)
        assert second.acquire(["app-mcp-wake:abc", "中文"]) is False
        assert not second.is_primary
        assert got.wait(5)
        assert received == [["app-mcp-wake:abc", "中文"]]
    finally:
        primary.close()
    assert not sock.exists()

    # 残留的套接字文件（无人监听）：新实例接管
    import socket as _socket

    stale = _socket.socket(_socket.AF_UNIX, _socket.SOCK_STREAM)
    stale.bind(str(sock))
    stale.close()
    again = SingleInstance("shop", path=sock)
    with again:
        assert again.acquire([], None) is True


def test_single_instance_wakes_client(tmp_path: Path):
    sock = tmp_path / "w.sock"
    rec = _Recorder()
    primary = SingleInstance("w", path=sock)
    with primary:
        assert primary.acquire([], rec.handle_wake)
        assert not SingleInstance("w", path=sock).acquire(["shop://app-mcp/wake?token=t9"])
        assert rec.event.wait(5)
        assert rec.args == [["shop://app-mcp/wake?token=t9"]]


def test_default_socket_path():
    p = default_socket_path("my-app")
    assert p.name == "my-app.sock" and "app-mcp" in str(p.parent)
    with pytest.raises(ValueError):
        default_socket_path("../evil")


# ---------------------------------------------------------------------------
# 延迟加载：没有 App 端原生库时 import app_mcp.hub / 纯 Python 辅助仍可用
# ---------------------------------------------------------------------------

_BLOCK_UNIFFI = """
import sys
sys.modules['app_mcp.app_mcp_uniffi'] = None   # 模拟生成物缺失
import app_mcp
from app_mcp import LifecyclePolicy, WakeDescriptor
import app_mcp.linux, app_mcp.single_instance
assert 'app_mcp._client' not in sys.modules
try:
    app_mcp.AppMcp
except ImportError:
    pass
else:
    raise SystemExit('AppMcp 不应可用')
try:
    import app_mcp.hub
except ImportError as e:
    if 'app_mcp_hub' in str(e):
        print('NO_HUB')
        raise SystemExit(0)
    raise
assert 'app_mcp._client' not in sys.modules
print('HUB_OK')
"""


def test_hub_import_without_app_native_library():
    r = subprocess.run(
        [sys.executable, "-c", _BLOCK_UNIFFI],
        env={**os.environ, "PYTHONPATH": str(SRC)},
        capture_output=True, text=True, timeout=60,
    )
    assert r.returncode == 0, r.stdout + r.stderr
    if "NO_HUB" in r.stdout:
        pytest.skip("没有 app_mcp_hub 生成物（纯 Python 部分已验证）")
    assert "HUB_OK" in r.stdout


def test_lazy_attributes():
    assert app_mcp.AppMcp is AppMcp
    assert app_mcp.WakeReason is ffi.WakeReason
    assert "AppMcp" in dir(app_mcp)
    with pytest.raises(AttributeError):
        app_mcp.NoSuchThing  # noqa: B018
