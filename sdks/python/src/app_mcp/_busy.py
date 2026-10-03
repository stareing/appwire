"""「用户正在操作」状态（spec/protocol.md 5.3，第 16 项 N6）：显式开关与引用计数作用域合并后交给核心。"""

from __future__ import annotations

import threading
from collections.abc import Callable, Iterator
from contextlib import contextmanager
from typing import Literal, Union

from . import app_mcp_uniffi as ffi
from ._client_convert import _enum_arg

BusyPolicyLike = Union[Literal["reject", "queue"], ffi.BusyPolicy]

_BUSY_POLICIES = {
    "reject": ffi.BusyPolicy.REJECT,
    "queue": ffi.BusyPolicy.QUEUE,
}


def _busy_policy(value: BusyPolicyLike) -> ffi.BusyPolicy:
    """@error 未知字符串或非 BusyPolicy / str 值时抛 ``ValueError``。"""
    if isinstance(value, ffi.BusyPolicy):
        return value
    if not isinstance(value, str):
        raise ValueError(f"未知的忙碌策略：{value!r}（可选 {sorted(_BUSY_POLICIES)}）")
    return _enum_arg(value, _BUSY_POLICIES, "忙碌策略")


class _BusyState:
    """合并 ``set_busy`` 开关与 ``busy()`` 作用域计数。

    @invariant 交给核心的值 = 开关 ∨ 作用域计数 > 0；``set_busy(False)`` 不结束仍在进行的作用域。
    @side-effect 每次变化都在锁内调用 ``apply``，保证多线程下推给核心的顺序与状态变化顺序一致。
    """

    def __init__(self, apply: Callable[[bool], None]) -> None:
        self._apply = apply
        self._lock = threading.Lock()
        self._manual = False
        self._scopes = 0

    def set(self, busy: bool) -> None:
        with self._lock:
            self._manual = bool(busy)
            self._push()

    @contextmanager
    def scope(self) -> Iterator[None]:
        with self._lock:
            self._scopes += 1
            self._push()
        try:
            yield
        finally:
            with self._lock:
                self._scopes -= 1
                self._push()

    def _push(self) -> None:
        self._apply(self._manual or self._scopes > 0)
