"""生命周期配置（spec/lifecycle.md）。纯 Python，不依赖原生库（``app_mcp.linux`` 等辅助模块可单独使用）。"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Literal

__all__ = ["LifecyclePolicy", "WakeDescriptor", "LIFECYCLE_MODES", "RESIDENCIES", "WAKE_KINDS"]

LifecycleModeName = Literal["persistent", "idle", "on-demand"]
ResidencyName = Literal["keep", "exit-when-idle", "exit-always"]
WakeKindName = Literal["uri", "aumid", "apple-event", "dbus", "android-intent", "web-url", "none"]

LIFECYCLE_MODES: frozenset[str] = frozenset({"persistent", "idle", "on-demand"})
RESIDENCIES: frozenset[str] = frozenset({"keep", "exit-when-idle", "exit-always"})
WAKE_KINDS: frozenset[str] = frozenset({"uri", "aumid", "apple-event", "dbus", "android-intent", "web-url", "none"})


def _norm(value: str) -> str:
    return value.strip().lower().replace("_", "-")


@dataclass(frozen=True)
class WakeDescriptor:
    """本实例的唤醒描述（spec/lifecycle.md 第 5 节），随 ``app/sleep`` 上报。

    - ``kind``：``"uri"`` / ``"dbus"`` / ``"apple-event"`` 等；
    - ``target``：定位信息（URI scheme、D-Bus 名等）；
    - ``background``：能否不把窗口带到前台就唤醒。
    """

    kind: WakeKindName | str
    target: str | None = None
    background: bool = False

    def __post_init__(self) -> None:
        kind = _norm(self.kind)
        if kind not in WAKE_KINDS:
            raise ValueError(f"未知的唤醒方式：{self.kind!r}（可选 {sorted(WAKE_KINDS)}）")
        object.__setattr__(self, "kind", kind)


@dataclass(frozen=True)
class LifecyclePolicy:
    """生命周期策略（spec/lifecycle.md 第 3 节）。时间单位为秒。

    - ``mode``：``"persistent"``（默认，不休眠）/ ``"idle"``（空闲 ``idle_timeout`` 后休眠）/
      ``"on-demand"``（启动时不连接，被唤醒或 ``connect_now()`` 时连接，任务完成后经过 ``grace`` 休眠）；
    - ``hidden_idle_timeout``：可见性为 hidden / frozen 时的空闲时间；
    - ``residency``：``"keep"`` / ``"exit-when-idle"`` / ``"exit-always"``，后两者在休眠后回调
      ``on_idle_exit``（由 App 决定是否退出进程）；
    - ``wake``：唤醒描述；Linux 可用 :func:`app_mcp.linux.dbus_wake_descriptor` 生成；
    - ``host_absent_retries``：``idle`` / ``on-demand`` 下连续多少次"Host 不在"后停止重连、进入 ``dormant``，
      0 = 一直重连（第 11 节 A2）；
    - ``legacy_timers``：回退到 4e 之前的定时器行为（第 11、13 节）；
    - ``merge_window``：调用 / 资源读取完成后的合并窗口，之后是否在线只由 Host 租约决定（第 13 节 B1）；
    - ``sleep_on_background``：``idle`` / ``on-demand`` 下进入后台（可见性 hidden / frozen）且空闲时立即休眠，
      不等租约（第 13 节 B4）。

    默认 ``persistent``（与核心一致）；Linux 桌面 App 有 D-Bus 唤醒时用 :func:`app_mcp.linux.dbus_lifecycle`。
    """

    mode: LifecycleModeName | str = "persistent"
    idle_timeout: float = 60.0
    hidden_idle_timeout: float = 15.0
    grace: float = 10.0
    residency: ResidencyName | str = "keep"
    wake: WakeDescriptor | None = None
    host_absent_retries: int = 3
    legacy_timers: bool = False
    merge_window: float = 2.0
    sleep_on_background: bool = False

    def __post_init__(self) -> None:
        mode = _norm(self.mode)
        if mode not in LIFECYCLE_MODES:
            raise ValueError(f"未知的生命周期模式：{self.mode!r}（可选 {sorted(LIFECYCLE_MODES)}）")
        residency = _norm(self.residency)
        if residency not in RESIDENCIES:
            raise ValueError(f"未知的驻留策略：{self.residency!r}（可选 {sorted(RESIDENCIES)}）")
        for name in ("idle_timeout", "hidden_idle_timeout", "grace", "merge_window", "host_absent_retries"):
            if getattr(self, name) < 0:
                raise ValueError(f"{name} 不能为负数")
        object.__setattr__(self, "mode", mode)
        object.__setattr__(self, "residency", residency)
