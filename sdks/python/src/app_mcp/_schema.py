"""从函数签名生成 JSON Schema，并把参数对象绑定为函数实参。

只覆盖常见类型：str、int、float、bool、list[T]、dict、Optional[T]、Literal[...]、
pydantic 模型（若已安装）。无法识别的注解生成 ``{}``（任意值）。需要精确控制时，
在 ``@client.tool(..., input_schema=...)`` 中显式给出 schema。
"""

from __future__ import annotations

import dataclasses
import enum
import inspect
import types
import typing
from typing import Any, Callable

try:  # pydantic 是可选依赖
    import pydantic as _pydantic
except ImportError:  # pragma: no cover - 取决于环境
    _pydantic = None

__all__ = ["schema_from_function", "ArgumentBinder", "is_pydantic_model"]

_CONTEXT_NAMES = ("ctx", "context")


def is_pydantic_model(tp: Any) -> bool:
    return _pydantic is not None and isinstance(tp, type) and issubclass(tp, _pydantic.BaseModel)


def _strip_optional(tp: Any) -> tuple[Any, bool]:
    origin = typing.get_origin(tp)
    if origin is typing.Union or origin is types.UnionType:
        args = [a for a in typing.get_args(tp) if a is not type(None)]
        if len(args) == 1 and len(args) != len(typing.get_args(tp)):
            return args[0], True
    return tp, False


def type_to_schema(tp: Any) -> dict[str, Any]:
    """把类型注解转换为 JSON Schema 片段。"""
    if tp is inspect.Parameter.empty or tp is Any:
        return {}
    tp, _ = _strip_optional(tp)
    if is_pydantic_model(tp):
        return tp.model_json_schema()
    if tp is str:
        return {"type": "string"}
    if tp is bool:
        return {"type": "boolean"}
    if tp is int:
        return {"type": "integer"}
    if tp is float:
        return {"type": "number"}
    if tp is dict:
        return {"type": "object"}
    if tp is list or tp is tuple or tp is set:
        return {"type": "array"}
    if isinstance(tp, type) and issubclass(tp, enum.Enum):
        return {"enum": [m.value for m in tp]}
    origin = typing.get_origin(tp)
    if origin is typing.Literal:
        return {"enum": list(typing.get_args(tp))}
    if origin in (list, tuple, set, typing.Sequence):
        args = typing.get_args(tp)
        schema: dict[str, Any] = {"type": "array"}
        if args and origin is not tuple:
            schema["items"] = type_to_schema(args[0])
        return schema
    if origin is dict:
        return {"type": "object"}
    if origin is typing.Union or origin is types.UnionType:
        return {"anyOf": [type_to_schema(a) for a in typing.get_args(tp)]}
    return {}


def _hints(fn: Callable[..., Any]) -> dict[str, Any]:
    try:
        return typing.get_type_hints(fn)
    except Exception:  # 前向引用无法解析时退回原始注解
        return dict(getattr(fn, "__annotations__", {}))


class ArgumentBinder:
    """描述函数如何接收参数：普通关键字参数、单个 pydantic 模型参数、上下文参数。"""

    def __init__(self, fn: Callable[..., Any], context_type: type | None = None) -> None:
        self.fn = fn
        sig = inspect.signature(fn)
        hints = _hints(fn)
        self.context_param: str | None = None
        self.params: dict[str, inspect.Parameter] = {}
        self.annotations: dict[str, Any] = {}
        self.var_keyword = False
        for name, p in sig.parameters.items():
            ann = hints.get(name, p.annotation)
            if (context_type is not None and ann is context_type) or (
                ann is inspect.Parameter.empty and name in _CONTEXT_NAMES
            ):
                self.context_param = name
                continue
            if p.kind is inspect.Parameter.VAR_KEYWORD:
                self.var_keyword = True
                continue
            if p.kind is inspect.Parameter.VAR_POSITIONAL:
                continue
            self.params[name] = p
            self.annotations[name] = ann
        # 只有一个参数且类型是 pydantic 模型：整个参数对象就是这个模型。
        self.model_param: str | None = None
        if len(self.params) == 1:
            (only,) = self.params
            if is_pydantic_model(self.annotations[only]):
                self.model_param = only

    def schema(self) -> dict[str, Any] | None:
        if self.model_param is not None:
            return self.annotations[self.model_param].model_json_schema()
        if not self.params:
            return None if not self.var_keyword else {"type": "object"}
        properties: dict[str, Any] = {}
        required: list[str] = []
        for name, p in self.params.items():
            ann = self.annotations[name]
            prop = type_to_schema(ann)
            _, optional = _strip_optional(ann)
            if p.default is not inspect.Parameter.empty:
                default = p.default
                if isinstance(default, enum.Enum):
                    default = default.value
                if default is None or isinstance(default, (str, int, float, bool, list, dict)):
                    prop = {**prop, "default": default}
            elif not optional:
                required.append(name)
            properties[name] = prop
        schema: dict[str, Any] = {"type": "object", "properties": properties}
        if required:
            schema["required"] = required
        if not self.var_keyword:
            schema["additionalProperties"] = False
        return schema

    def bind(self, args: dict[str, Any], context: Any) -> dict[str, Any]:
        """把参数对象转换为关键字实参。类型不符时抛 ``ValueError``（映射为 INVALID_INPUT）。"""
        kwargs: dict[str, Any] = {}
        if self.model_param is not None:
            kwargs[self.model_param] = self._convert(self.annotations[self.model_param], args)
        else:
            for name in self.params:
                if name in args:
                    kwargs[name] = self._convert(self.annotations[name], args[name])
            if self.var_keyword:
                for key, value in args.items():
                    if key not in self.params:
                        kwargs[key] = value
        if self.context_param is not None:
            kwargs[self.context_param] = context
        return kwargs

    @staticmethod
    def _convert(ann: Any, value: Any) -> Any:
        tp, _ = _strip_optional(ann)
        if value is None:
            return None
        if is_pydantic_model(tp):
            try:
                return tp.model_validate(value)
            except Exception as e:  # pydantic.ValidationError
                raise ValueError(str(e)) from e
        if isinstance(tp, type) and issubclass(tp, enum.Enum):
            return tp(value)
        if dataclasses.is_dataclass(tp) and isinstance(tp, type) and isinstance(value, dict):
            return tp(**value)
        return value


def schema_from_function(fn: Callable[..., Any], context_type: type | None = None) -> dict[str, Any] | None:
    """从函数签名生成 inputSchema；函数没有参数时返回 ``None``。"""
    return ArgumentBinder(fn, context_type).schema()


def to_jsonable(value: Any) -> Any:
    """``json.dumps`` 的 ``default``：支持 pydantic 模型、dataclass、枚举、集合。"""
    if _pydantic is not None and isinstance(value, _pydantic.BaseModel):
        return value.model_dump(mode="json")
    if dataclasses.is_dataclass(value) and not isinstance(value, type):
        return dataclasses.asdict(value)
    if isinstance(value, enum.Enum):
        return value.value
    if isinstance(value, (set, frozenset, tuple)):
        return list(value)
    raise TypeError(f"类型 {type(value).__name__} 无法序列化为 JSON")
