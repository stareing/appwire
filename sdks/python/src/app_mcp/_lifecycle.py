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
    - ``wake``：唤醒描述；Linux 可用 :func:`app_mcp.linux.dbus_wake_descriptor` 生成。
    """

    mode: LifecycleModeName | str = "persistent"
    idle_timeout: float = 60.0
    hidden_idle_timeout: float = 15.0
    grace: float = 10.0
    residency: ResidencyName | str = "keep"
    wake: WakeDescriptor | None = None

    def __post_init__(self) -> None:
        mode = _norm(self.mode)
        if mode not in LIFECYCLE_MODES:
            raise ValueError(f"未知的生命周期模式：{self.mode!r}（可选 {sorted(LIFECYCLE_MODES)}）")
        residency = _norm(self.residency)
        if residency not in RESIDENCIES:
            raise ValueError(f"未知的驻留策略：{self.residency!r}（可选 {sorted(RESIDENCIES)}）")
        for name in ("idle_timeout", "hidden_idle_timeout", "grace"):
            if getattr(self, name) < 0:
                raise ValueError(f"{name} 不能为负数")
        object.__setattr__(self, "mode", mode)
        object.__setattr__(self, "residency", residency)
