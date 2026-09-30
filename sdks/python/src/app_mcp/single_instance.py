"""单实例转发（Unix 域套接字）：第二个实例把命令行参数交给已运行的实例后退出。

用于 URI / 命令行唤醒：Host 以 ``app --wake app-mcp-wake:<token>`` 或 ``xdg-open shop://app-mcp/wake?token=…``
拉起 App 时，若已有实例在运行，新进程把参数转发给它（由它调用 ``client.handle_wake``），自身立即退出。

::

    import sys
    from app_mcp.single_instance import SingleInstance

    instance = SingleInstance("shop")
    if not instance.acquire(sys.argv[1:], on_argv=lambda argv: client.handle_wake(argv)):
        sys.exit(0)            # 已转发给正在运行的实例
    client.handle_wake(sys.argv[1:])   # 本进程由唤醒冷启动时
    client.start()

安全：套接字放在只有本用户可访问的目录（``$XDG_RUNTIME_DIR`` 或 ``/tmp/app-mcp-<uid>``，权限 0700），
套接字文件权限 0600；Linux 上另外用 ``SO_PEERCRED`` 拒绝其他用户的连接。
消息为单行 JSON 字符串数组（上限 64 KiB）。接收线程阻塞在 ``accept`` 上，不轮询。
本模块不依赖原生库。
"""

from __future__ import annotations

import errno
import json
import logging
import os
import re
import socket
import struct
import tempfile
import threading
from collections.abc import Callable, Sequence
from pathlib import Path

__all__ = ["SingleInstance", "default_socket_path"]

logger = logging.getLogger("app_mcp.single_instance")

_NAME = re.compile(r"^[A-Za-z0-9._-]{1,64}$")
_MAX_MESSAGE = 64 * 1024


def default_socket_path(name: str) -> Path:
    """``$XDG_RUNTIME_DIR/app-mcp/<name>.sock``，没有时 ``<tmp>/app-mcp-<uid>/<name>.sock``。"""
    if not _NAME.match(name):
        raise ValueError(f"实例名只能包含 [A-Za-z0-9._-]（1–64 个字符）：{name!r}")
    runtime = os.environ.get("XDG_RUNTIME_DIR")
    if runtime and os.path.isdir(runtime):
        base = Path(runtime) / "app-mcp"
    else:
        base = Path(tempfile.gettempdir()) / f"app-mcp-{os.getuid()}"
    return base / f"{name}.sock"


def _prepare_dir(directory: Path) -> None:
    directory.mkdir(mode=0o700, parents=True, exist_ok=True)
    st = directory.stat()
    if st.st_uid != os.getuid():
        raise PermissionError(f"{directory} 不属于当前用户，拒绝使用")
    if st.st_mode & 0o077:
        os.chmod(directory, 0o700)


def _peer_uid(conn: socket.socket) -> int | None:
    so_peercred = getattr(socket, "SO_PEERCRED", None)
    if so_peercred is None:  # macOS 等：依赖目录权限
        return None
    creds = conn.getsockopt(socket.SOL_SOCKET, so_peercred, struct.calcsize("3i"))
    _pid, uid, _gid = struct.unpack("3i", creds)
    return uid


class SingleInstance:
    """同名只允许一个主实例；其他实例通过 :meth:`acquire` 把参数转发给主实例。"""

    def __init__(self, name: str, *, path: str | os.PathLike[str] | None = None) -> None:
        self.name = name
        self.path = Path(path) if path is not None else default_socket_path(name)
        self._server: socket.socket | None = None
        self._thread: threading.Thread | None = None
        self._on_argv: Callable[[list[str]], object] | None = None
        self._closed = threading.Event()

    @property
    def is_primary(self) -> bool:
        return self._server is not None

    def acquire(
        self,
        argv: Sequence[str] = (),
        on_argv: Callable[[list[str]], object] | None = None,
        *,
        timeout: float = 2.0,
    ) -> bool:
        """成为主实例返回 ``True``（开始监听，之后收到的参数交给 ``on_argv``，在接收线程上调用）；
        已有主实例时把 ``argv`` 转发给它并返回 ``False``（调用方应退出）。"""
        if self._server is not None:
            return True
        _prepare_dir(self.path.parent)
        for _ in range(3):
            if self._forward(list(argv), timeout):
                return False
            server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            try:
                server.bind(str(self.path))
            except OSError as e:
                server.close()
                if e.errno != errno.EADDRINUSE:
                    raise
                # 残留的套接字文件（上次异常退出）：确认无人监听后删除重试。
                if self._forward(list(argv), timeout):
                    return False
                self.path.unlink(missing_ok=True)
                continue
            os.chmod(self.path, 0o600)
            server.listen(8)
            self._server = server
            self._on_argv = on_argv
            self._thread = threading.Thread(target=self._serve, name=f"app-mcp-single-{self.name}", daemon=True)
            self._thread.start()
            return True
        raise RuntimeError(f"无法成为 {self.name} 的主实例，也无法连接到已有实例：{self.path}")

    def close(self) -> None:
        """停止监听并删除套接字文件（仅主实例）。"""
        if self._server is None or self._closed.is_set():
            return
        self._closed.set()
        try:
            self._server.shutdown(socket.SHUT_RDWR)
        except OSError:
            pass
        self._server.close()
        if self._thread is not None:
            self._thread.join(timeout=2)
        try:
            self.path.unlink(missing_ok=True)
        except OSError:
            pass

    def __enter__(self) -> SingleInstance:
        return self

    def __exit__(self, *exc: object) -> None:
        self.close()

    # -- 内部 ------------------------------------------------------------------

    def _forward(self, argv: list[str], timeout: float) -> bool:
        """连接主实例并发送参数；没有主实例（不存在 / 拒绝连接）返回 ``False``。"""
        if not self.path.exists():
            return False
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        sock.settimeout(timeout)
        try:
            try:
                sock.connect(str(self.path))
            except (FileNotFoundError, ConnectionRefusedError):
                return False
            sock.sendall(json.dumps(argv, ensure_ascii=False).encode("utf-8") + b"\n")
            reply = sock.makefile("rb").readline()
            if reply.strip() != b"ok":
                logger.warning("主实例未确认转发：%r", reply)
            return True
        finally:
            sock.close()

    def _serve(self) -> None:
        server = self._server
        assert server is not None
        while not self._closed.is_set():
            try:
                conn, _ = server.accept()
            except OSError:
                return
            with conn:
                try:
                    self._handle(conn)
                except Exception:
                    logger.exception("处理转发参数失败")

    def _handle(self, conn: socket.socket) -> None:
        uid = _peer_uid(conn)
        if uid is not None and uid != os.getuid():
            logger.warning("拒绝来自其他用户（uid=%s）的转发", uid)
            return
        conn.settimeout(2.0)
        data = conn.makefile("rb").readline(_MAX_MESSAGE + 1)
        if len(data) > _MAX_MESSAGE:
            return
        argv = json.loads(data.decode("utf-8"))
        if not isinstance(argv, list) or not all(isinstance(a, str) for a in argv):
            return
        conn.sendall(b"ok\n")
        if self._on_argv is not None:
            self._on_argv(argv)
