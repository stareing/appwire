"""按名寻址的最小 Python App（spec/naming.md）：在系统名字服务登记、不主动连接 Hub，由 Hub 拨号时接受通道；
由激活启动（参数 ``--app-mcp-activation``）时通道关闭后退出，进程交还系统。

::

    PYTHONPATH=src python examples/named_app.py                 # 直接运行：登记名字，常驻直到 Ctrl-C
    app-mcp-host app install --app-id py-named --exec <可执行文件>  # 登记激活方式（Linux：D-Bus 激活文件）
    app-mcp-host serve --name-service                            # Hub 按名发现、调用时拨号

``--exec`` 需要一个可执行文件（如带 ``#!/usr/bin/env python3`` 的脚本），激活时追加 ``--app-mcp-activation``。

环境变量：``APP_MCP_APP_ID``（默认 ``py-named``）；``APP_MCP_EVENT_LOG``（可选，追加 ``start <pid>`` / ``exit <pid>``
行，测试据此核对激活次数与退出）。工具：``echo``（原样返回 ``text``）、``pid``（返回进程号）。
"""

from __future__ import annotations

import os
import threading

from app_mcp import AppMcp, LifecyclePolicy


def note(event: str) -> None:
    path = os.environ.get("APP_MCP_EVENT_LOG")
    if path:
        with open(path, "a", encoding="utf-8") as f:
            f.write(f"{event} {os.getpid()}\n")


def main() -> None:
    note("start")
    idle_exit = threading.Event()
    client = AppMcp(
        os.environ.get("APP_MCP_APP_ID", "py-named"),
        "按名寻址示例（Python）",
        lifecycle=LifecyclePolicy(mode="on-demand", residency="exit-when-idle"),
        register_name=True,
        on_idle_exit=idle_exit.set,
    )

    @client.tool("echo", description="原样返回 text")
    def echo(text: str) -> dict:
        return {"echo": text}

    @client.tool("pid", description="返回进程号")
    def pid() -> dict:
        return {"pid": os.getpid()}

    client.start()
    try:
        # 由激活启动：通道关闭后收到 on_idle_exit 即退出；直接运行时不会收到，一直等待。
        idle_exit.wait()
    except KeyboardInterrupt:
        pass
    client.close()
    note("exit")


if __name__ == "__main__":
    main()
