"""``_client`` 的工具与资源句柄（纯移动自 ``_client.py``）。"""

from __future__ import annotations

from collections.abc import Sequence
from typing import Any

from . import app_mcp_uniffi as ffi
from ._client_convert import (
    _UNSET,
    ActivationLike,
    CacheLike,
    DeprecationLike,
    RiskLike,
    SurfaceLike,
    ToolAnnotationsLike,
    _activation,
    _cache,
    _deprecation,
    _implements,
    _risk,
    _schema_json,
    _surface,
    _tool_annotations,
    _Unset,
)


# ---------------------------------------------------------------------------
# 句柄
# ---------------------------------------------------------------------------


class ToolHandle:
    """已注册的工具。"""

    def __init__(self, inner: ffi.Tool, spec: ffi.ToolSpec) -> None:
        self._inner = inner
        self._spec = spec

    @property
    def name(self) -> str:
        return self._inner.name()

    def set_enabled(self, enabled: bool) -> None:
        self._inner.set_enabled(enabled)
        # @why update() 整体替换定义，须记住当前启用状态，否则之后的 update 会把它改回去
        self._spec = _replace_spec(self._spec, enabled=enabled)

    def update(
        self,
        *,
        description: str | _Unset = _UNSET,
        input_schema: dict[str, Any] | str | None | _Unset = _UNSET,
        risk: RiskLike | None | _Unset = _UNSET,
        activation: ActivationLike | None | _Unset = _UNSET,
        title: str | None | _Unset = _UNSET,
        annotations: ToolAnnotationsLike | None | _Unset = _UNSET,
        output_schema: dict[str, Any] | str | None | _Unset = _UNSET,
        surface: SurfaceLike | None | _Unset = _UNSET,
        page: str | None | _Unset = _UNSET,
        background_tool: str | None | _Unset = _UNSET,
        concurrency: int | _Unset = _UNSET,
        exclusive: str | None | _Unset = _UNSET,
        implements: Sequence[str] | _Unset = _UNSET,
        cache: CacheLike | None | _Unset = _UNSET,
        deprecated: DeprecationLike | None | _Unset = _UNSET,
        undoable: bool | _Unset = _UNSET,
    ) -> None:
        """修改定义：未给出的字段保持不变；显式传 ``None`` 清除该声明（恢复注册时的缺省）。

        ``input_schema=None`` 为无参数，``risk=None`` 为缺省风险，``surface=None`` 为 ``"app"``，``title`` /
        ``activation`` / ``annotations`` / ``output_schema`` / ``page`` / ``background_tool`` /
        ``exclusive`` 为 ``None`` 时清除声明。``description`` 不可清除；``concurrency=0`` 为不单独限制；``implements=[]`` 清除意图声明；
        ``cache`` 为 ``None`` 时清除结果缓存声明；``deprecated`` 为 ``None`` 时取消弃用；
        ``undoable=False`` 取消可撤销声明。
        """
        s = self._spec
        spec = _replace_spec(
            s,
            description=s.description if description is _UNSET else description,
            input_schema_json=s.input_schema_json if input_schema is _UNSET else _schema_json(input_schema),
            risk=s.risk if risk is _UNSET else _risk(risk),
            activation=s.activation if activation is _UNSET else _activation(activation),
            title=s.title if title is _UNSET else title,
            annotations=s.annotations if annotations is _UNSET else _tool_annotations(annotations),
            output_schema_json=s.output_schema_json if output_schema is _UNSET else _schema_json(output_schema),
            surface=s.surface if surface is _UNSET else _surface(surface),
            page=s.page if page is _UNSET else page,
            background_tool=s.background_tool if background_tool is _UNSET else background_tool,
            concurrency=s.concurrency if concurrency is _UNSET else concurrency,
            exclusive=s.exclusive if exclusive is _UNSET else exclusive,
            implements=s.implements if implements is _UNSET else _implements(implements),
            cache=s.cache if cache is _UNSET else _cache(cache),
            deprecated=s.deprecated if deprecated is _UNSET else _deprecation(deprecated),
            undoable=s.undoable if undoable is _UNSET else undoable,
        )
        self._inner.update(spec)
        self._spec = spec

    def dispose(self) -> None:
        self._inner.dispose()


def _replace_spec(spec: ffi.ToolSpec, **changes: Any) -> ffi.ToolSpec:
    """复制 ``ToolSpec`` 并替换给出的字段（uniffi 记录不是 dataclass）。

    @why 从实例属性复制全部字段，绑定新增字段时不必在此逐个登记（曾因漏登记而在 ``update`` 时丢失声明）。
    """
    fields = dict(vars(spec))
    fields.update(changes)
    return ffi.ToolSpec(**fields)


class ResourceHandle:
    """已注册的资源。"""

    def __init__(self, inner: ffi.Resource) -> None:
        self._inner = inner

    @property
    def name(self) -> str:
        return self._inner.name()

    def notify_changed(self) -> None:
        self._inner.notify_changed()

    def dispose(self) -> None:
        self._inner.dispose()
