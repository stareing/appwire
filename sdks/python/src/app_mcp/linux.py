"""Linux 唤醒辅助：D-Bus ``org.freedesktop.Application``（spec/lifecycle.md 第 5 节）。

Host 唤醒休眠实例时调用::

    org.freedesktop.Application.ActivateAction("app-mcp-wake", [<token>], {})

对象路径为总线名把 ``.`` 换成 ``/``（如 ``org.example.Shop`` → ``/org/example/Shop``）。
若为总线名安装了 D-Bus 服务文件（:func:`dbus_service_file` / :func:`install_dbus_service`），
进程已退出时 dbus-daemon 会自动拉起 App，App 启动后同样会收到这次 ``ActivateAction``。

用法::

    from app_mcp import AppMcp, LifecyclePolicy
    from app_mcp.linux import dbus_wake_descriptor, serve_dbus_wake

    BUS = "org.example.Shop"
    client = AppMcp("shop", "Shop", lifecycle=LifecyclePolicy(mode="idle", wake=dbus_wake_descriptor(BUS)))
    service = serve_dbus_wake(client, BUS)     # 需要可选依赖 jeepney（pip install jeepney）
    client.start()

GTK（``Gio.Application``）/ Qt（``QDBusAbstractAdaptor``）应用已有自己的 D-Bus 对象时不必用
:func:`serve_dbus_wake`：在框架里注册名为 ``app-mcp-wake``、参数类型为字符串的 action，
回调中调用 ``client.handle_wake(wake_arg_from_action("app-mcp-wake", [token]))`` 即可
（``Gio.SimpleAction.new("app-mcp-wake", GLib.VariantType("s"))``）。

本模块不依赖原生库，只在调用 :func:`serve_dbus_wake` 时需要 jeepney。
"""

from __future__ import annotations

import logging
import os
import re
import shlex
import socket
import threading
from collections.abc import Callable, Sequence
from pathlib import Path
from typing import Any, Protocol

from ._lifecycle import WakeDescriptor

__all__ = [
    "WAKE_ACTION",
    "DBusWakeService",
    "dbus_object_path",
    "dbus_service_file",
    "dbus_wake_descriptor",
    "install_dbus_service",
    "serve_dbus_wake",
    "wake_arg_from_action",
]

logger = logging.getLogger("app_mcp.linux")

WAKE_ACTION = "app-mcp-wake"
"""Host 调用的 action 名。"""

APPLICATION_IFACE = "org.freedesktop.Application"

_BUS_NAME = re.compile(r"^[A-Za-z_][A-Za-z0-9_-]*(\.[A-Za-z_][A-Za-z0-9_-]*)+$")

_INSTALL_HINT = (
    "D-Bus 唤醒服务需要可选依赖 jeepney：pip install jeepney。"
    "也可以在 GTK / Qt 的 D-Bus 对象上自行注册 action 'app-mcp-wake'，"
    "回调中调用 client.handle_wake(wake_arg_from_action(name, parameters))。"
)


class _Wakeable(Protocol):
    def handle_wake(self, argv: str | Sequence[str]) -> bool: ...


def _check_bus_name(bus_name: str) -> None:
    if len(bus_name) > 255 or not _BUS_NAME.match(bus_name):
        raise ValueError(f"不是合法的 D-Bus 总线名：{bus_name!r}（形如 org.example.App）")


def dbus_object_path(bus_name: str) -> str:
    """``org.example.Shop`` → ``/org/example/Shop``（``-`` 换成 ``_``，与 GApplication 一致）。"""
    _check_bus_name(bus_name)
    return "/" + bus_name.replace(".", "/").replace("-", "_")


def dbus_wake_descriptor(bus_name: str) -> WakeDescriptor:
    """本实例的唤醒描述：``kind="dbus"``、``target=<总线名>``；D-Bus 激活不需要把窗口带到前台。"""
    _check_bus_name(bus_name)
    return WakeDescriptor(kind="dbus", target=bus_name, background=True)


def dbus_service_file(bus_name: str, exec_path: str | Sequence[str]) -> str:
    """生成 D-Bus 会话服务文件（``<总线名>.service``）的文本。

    ``exec_path`` 为启动命令（字符串或参数列表，如 ``[sys.executable, "/opt/shop/main.py"]``）。
    安装到 ``~/.local/share/dbus-1/services/``（:func:`install_dbus_service`）或
    ``/usr/share/dbus-1/services/`` 后，dbus-daemon 在有人调用该总线名时自动拉起 App。
    """
    _check_bus_name(bus_name)
    if isinstance(exec_path, str):
        command = exec_path
    else:
        if not exec_path:
            raise ValueError("exec_path 不能为空")
        command = shlex.join(str(a) for a in exec_path)
    if not command.strip() or "\n" in command:
        raise ValueError(f"exec_path 不合法：{exec_path!r}")
    return f"[D-BUS Service]\nName={bus_name}\nExec={command}\n"


def install_dbus_service(
    bus_name: str,
    exec_path: str | Sequence[str],
    directory: str | os.PathLike[str] | None = None,
) -> Path:
    """把服务文件写入 ``directory``（默认 ``$XDG_DATA_HOME/dbus-1/services``），返回文件路径。"""
    if directory is None:
        data_home = os.environ.get("XDG_DATA_HOME") or os.path.join(Path.home(), ".local", "share")
        directory = os.path.join(data_home, "dbus-1", "services")
    target_dir = Path(directory)
    target_dir.mkdir(parents=True, exist_ok=True)
    path = target_dir / f"{bus_name}.service"
    path.write_text(dbus_service_file(bus_name, exec_path), encoding="utf-8")
    return path


def wake_arg_from_action(action_name: str, parameters: Sequence[Any]) -> str | None:
    """把 ``ActivateAction`` 的参数转换为 ``handle_wake`` 的参数（``app-mcp-wake:<token>``）。

    ``parameters`` 为 ``av``：元素可以是字符串，或 jeepney 反序列化得到的 ``(签名, 值)`` 二元组。
    不是本 SDK 的 action 时返回 ``None``。
    """
    if action_name != WAKE_ACTION or not parameters:
        return None
    first = parameters[0]
    if isinstance(first, tuple) and len(first) == 2:
        first = first[1]
    if not isinstance(first, str) or not first:
        return None
    return first if first.startswith(WAKE_ACTION + ":") else f"{WAKE_ACTION}:{first}"


_INTROSPECT_XML = """<!DOCTYPE node PUBLIC "-//freedesktop//DTD D-BUS Object Introspection 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/introspect.dtd">
<node>
  <interface name="org.freedesktop.Application">
    <method name="Activate"><arg type="a{sv}" name="platform_data" direction="in"/></method>
    <method name="Open">
      <arg type="as" name="uris" direction="in"/>
      <arg type="s" name="hint" direction="in"/>
      <arg type="a{sv}" name="platform_data" direction="in"/>
    </method>
    <method name="ActivateAction">
      <arg type="s" name="action_name" direction="in"/>
      <arg type="av" name="parameter" direction="in"/>
      <arg type="a{sv}" name="platform_data" direction="in"/>
    </method>
  </interface>
  <interface name="org.freedesktop.DBus.Introspectable">
    <method name="Introspect"><arg type="s" name="xml" direction="out"/></method>
  </interface>
</node>
"""


class DBusWakeService:
    """在会话总线上占用总线名并导出 ``org.freedesktop.Application``（基于 jeepney，后台线程）。

    - ``ActivateAction("app-mcp-wake", [token], {})`` → ``client.handle_wake("app-mcp-wake:<token>")``；
    - ``Open([uri, ...], hint, {})`` → 逐个交给 ``client.handle_wake``（URI 唤醒）；
    - ``Activate({})`` → 调用 ``on_activate``（若提供），例如把窗口带到前台。

    接收线程阻塞在 socket 上，不轮询。``close()`` 释放总线名并结束线程。
    """

    def __init__(
        self,
        client: _Wakeable,
        bus_name: str,
        *,
        on_activate: Callable[[], None] | None = None,
        bus: str = "SESSION",
    ) -> None:
        _check_bus_name(bus_name)
        try:
            from jeepney.bus_messages import message_bus
            from jeepney.io.blocking import open_dbus_connection
        except ImportError as e:
            raise ImportError(_INSTALL_HINT) from e

        self.bus_name = bus_name
        self.object_path = dbus_object_path(bus_name)
        self._client = client
        self._on_activate = on_activate
        self._closed = threading.Event()
        self._conn = open_dbus_connection(bus=bus)
        # DBUS_NAME_FLAG_DO_NOT_QUEUE = 4；返回 1 = 成为主拥有者
        reply = self._conn.send_and_get_reply(message_bus.RequestName(bus_name, 4), timeout=5)
        code = reply.body[0] if reply.body else 0
        if code not in (1, 4):  # 1 = PRIMARY_OWNER，4 = ALREADY_OWNER
            self._conn.close()
            raise RuntimeError(f"无法占用 D-Bus 总线名 {bus_name}（可能已有实例运行，返回码 {code}）")
        self._thread = threading.Thread(target=self._run, name="app-mcp-dbus", daemon=True)
        self._thread.start()

    def close(self) -> None:
        if self._closed.is_set():
            return
        self._closed.set()
        try:
            self._conn.sock.shutdown(socket.SHUT_RDWR)  # 让接收线程从阻塞的 recv 中返回
        except OSError:
            pass
        self._thread.join(timeout=2)
        try:
            self._conn.close()
        except OSError:
            pass

    def __enter__(self) -> DBusWakeService:
        return self

    def __exit__(self, *exc: object) -> None:
        self.close()

    # -- 内部 ------------------------------------------------------------------

    def _run(self) -> None:
        from jeepney import HeaderFields, MessageType

        while not self._closed.is_set():
            try:
                msg = self._conn.receive()
            except Exception:  # 连接关闭或出错
                if not self._closed.is_set():
                    logger.exception("D-Bus 接收失败，唤醒服务停止")
                return
            if msg.header.message_type != MessageType.method_call:
                continue
            fields = msg.header.fields
            try:
                self._handle(
                    msg,
                    fields.get(HeaderFields.path),
                    fields.get(HeaderFields.interface),
                    fields.get(HeaderFields.member),
                )
            except Exception:
                logger.exception("处理 D-Bus 调用失败")

    def _handle(self, msg: Any, path: str | None, iface: str | None, member: str | None) -> None:
        from jeepney import new_error, new_method_return

        if path != self.object_path:
            self._conn.send(new_error(msg, "org.freedesktop.DBus.Error.UnknownObject", "s", (f"未知对象 {path}",)))
            return
        if member == "Introspect" and iface in (None, "org.freedesktop.DBus.Introspectable"):
            self._conn.send(new_method_return(msg, "s", (_INTROSPECT_XML,)))
            return
        if iface not in (None, APPLICATION_IFACE):
            self._conn.send(new_error(msg, "org.freedesktop.DBus.Error.UnknownInterface", "s", (f"未知接口 {iface}",)))
            return
        if member == "ActivateAction":
            action, params = msg.body[0], msg.body[1]
            arg = wake_arg_from_action(action, params)
            self._conn.send(new_method_return(msg))
            if arg is not None:
                self._client.handle_wake(arg)
            else:
                logger.debug("忽略 D-Bus action %r", action)
        elif member == "Open":
            uris = list(msg.body[0])
            self._conn.send(new_method_return(msg))
            for uri in uris:
                self._client.handle_wake(uri)
        elif member == "Activate":
            self._conn.send(new_method_return(msg))
            if self._on_activate is not None:
                self._on_activate()
        else:
            self._conn.send(new_error(msg, "org.freedesktop.DBus.Error.UnknownMethod", "s", (f"未知方法 {member}",)))


def serve_dbus_wake(
    client: _Wakeable,
    bus_name: str,
    *,
    on_activate: Callable[[], None] | None = None,
    bus: str = "SESSION",
) -> DBusWakeService:
    """启动 :class:`DBusWakeService`。没有 jeepney 时抛 ``ImportError``（附安装说明）。"""
    return DBusWakeService(client, bus_name, on_activate=on_activate, bus=bus)
