"""兜底工具的参数解析、输入 schema 与错误（spec/ui-fallback.md 第 2 节、2.1、7.1），与 UI 框架无关。"""

from __future__ import annotations

import enum
import math
from dataclasses import dataclass
from typing import Any, Union

from .._client import ToolCallError
from . import _format as fmt

__all__ = [
    "UiKey",
    "UiScrollDirection",
    "FillText",
    "FillNumber",
    "FillBool",
    "UiFillValue",
    "invalid",
    "stale",
    "hidden",
    "disabled",
    "secure",
    "unsupported",
    "arg_string",
    "arg_int",
    "arg_ref",
    "arg_value",
    "parse_key",
    "parse_direction",
    "as_text",
    "as_bool",
    "as_number",
    "schemas",
]


class UiKey(enum.Enum):
    """支持的按键（spec/ui-fallback.md 2.1）。"""

    ENTER = "Enter"
    ESCAPE = "Escape"
    TAB = "Tab"
    SHIFT_TAB = "Shift+Tab"
    SPACE = "Space"


class UiScrollDirection(enum.Enum):
    """查看方向（spec/ui-fallback.md 第 2 节：``DOWN`` = 向下翻看更多内容）。"""

    UP = "up"
    DOWN = "down"
    LEFT = "left"
    RIGHT = "right"


@dataclass(frozen=True)
class FillText:
    text: str


@dataclass(frozen=True)
class FillNumber:
    number: float


@dataclass(frozen=True)
class FillBool:
    value: bool


UiFillValue = Union[FillText, FillNumber, FillBool]
"""``ui.fill`` 的值：文本、数字或布尔。"""


def invalid(message: str, reference: str | None = None, reason: str | None = None) -> ToolCallError:
    details = {k: v for k, v in (("ref", reference), ("reason", reason)) if v is not None}
    return ToolCallError("INVALID_INPUT", message, details or None)


def stale(reference: str, prefix: str) -> ToolCallError:
    return invalid(f"引用 {reference} 已失效，请重新调用 {prefix}.outline", reference)


def hidden(described: str, reference: str, prefix: str) -> ToolCallError:
    return invalid(f"{described}当前不可见，请重新调用 {prefix}.outline", reference, "hidden")


def disabled(described: str, reference: str) -> ToolCallError:
    return invalid(f"{described}已禁用，当前无法操作", reference, "TOOL_DISABLED")


def secure(described: str, reference: str | None) -> ToolCallError:
    return invalid(f"{described}是密码类控件，兜底工具不填写", reference, "secure")


def unsupported(described: str, reference: str | None, why: str | None = None) -> ToolCallError:
    return invalid(f"{described}不支持该操作" + (f"：{why}" if why else ""), reference, "unsupported")


def arg_string(args: dict[str, Any], key: str) -> str | None:
    v = args.get(key)
    if v is None:
        return None
    if not isinstance(v, str):
        raise invalid(f"{key} 应为字符串")
    return v


def arg_int(args: dict[str, Any], key: str) -> int | None:
    v = args.get(key)
    if v is None:
        return None
    if isinstance(v, bool) or not isinstance(v, (int, float)) or not math.isfinite(v):
        raise invalid(f"{key} 应为正整数")
    return int(math.floor(v))


def arg_ref(args: dict[str, Any], key: str, required: bool = True) -> str | None:
    v = args.get(key)
    if v is None or v == "":
        if required:
            raise invalid(f'缺少参数 {key}（控件引用，如 "e12"）')
        return None
    s = v.strip() if isinstance(v, str) else None
    if s is None or not fmt.REF_PATTERN.match(s):
        raise invalid(f'{key} 应为控件引用，如 "e12"')
    return s


def arg_value(args: dict[str, Any]) -> UiFillValue:
    if "value" not in args:
        raise invalid("缺少参数 value")
    v = args["value"]
    if isinstance(v, str):
        return FillText(v)
    if isinstance(v, bool):
        return FillBool(v)
    if isinstance(v, (int, float)) and math.isfinite(v):
        return FillNumber(float(v))
    raise invalid("value 应为文本、数字或布尔")


_KEYS: dict[str, UiKey] = {
    "enter": UiKey.ENTER, "return": UiKey.ENTER, "escape": UiKey.ESCAPE, "esc": UiKey.ESCAPE,
    "tab": UiKey.TAB, "shift+tab": UiKey.SHIFT_TAB, " ": UiKey.SPACE, "space": UiKey.SPACE,
}  # fmt: skip


def parse_key(key: str) -> UiKey:
    found = _KEYS.get(key if key == " " else key.strip().lower())
    if found is None:
        raise invalid(f"不支持的按键「{key}」；支持 Enter、Escape、Tab、Shift+Tab、Space")
    return found


def parse_direction(direction: str | None) -> UiScrollDirection | None:
    if direction is None:
        return None
    try:
        return UiScrollDirection(direction)
    except ValueError:
        raise invalid("direction 应为 up / down / left / right") from None


def as_text(value: UiFillValue, described: str, reference: str | None) -> str:
    """文本值：字符串原样，数字转十进制文本（整数不带小数点）。"""
    if isinstance(value, FillText):
        return value.text
    if isinstance(value, FillNumber):
        n = value.number
        return str(int(n)) if n.is_integer() and abs(n) < 1e15 else repr(n)
    raise invalid(f"{described}需要文本值", reference)


def as_bool(value: UiFillValue, described: str, reference: str | None) -> bool:
    if isinstance(value, FillBool):
        return value.value
    raise invalid(f"{described}需要 true / false", reference)


def as_number(value: UiFillValue, described: str, reference: str | None) -> float:
    if isinstance(value, FillNumber):
        return value.number
    if isinstance(value, FillText):
        try:
            n = float(value.text.strip())
        except ValueError:
            n = math.nan
        if math.isfinite(n):
            return n
    raise invalid(f"{described}需要数字", reference)


def _ref_prop(description: str = '控件引用，如 "e12"（来自 outline）') -> dict[str, Any]:
    return {"type": "string", "description": description}


def _obj(required: list[str], properties: dict[str, Any]) -> dict[str, Any]:
    out: dict[str, Any] = {"type": "object", "properties": properties}
    if required:
        out["required"] = required
    out["additionalProperties"] = False
    return out


def schemas(max_items: int) -> dict[str, dict[str, Any]]:
    """各工具的输入 schema（spec/ui-fallback.md 第 2 节），键为工具名后缀。"""
    return {
        "outline": _obj(
            [],
            {
                "query": {"type": "string", "description": "按名称模糊过滤（空格分隔多个词，全部匹配）"},
                "within": _ref_prop('只列出该引用分组的子树，如 "e40"'),
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": fmt.LIMIT_MAX,
                    "description": f"最多列出的控件数，默认 {max_items}",
                },
            },
        ),
        "click": _obj(["ref"], {"ref": _ref_prop()}),
        "fill": _obj(
            ["ref", "value"],
            {
                "ref": _ref_prop(),
                "value": {
                    "description": "文本、数字（滑块 / 数字框）、布尔（复选框 / 开关 / 单选框）或选项文本（下拉框）",
                    "anyOf": [{"type": "string"}, {"type": "number"}, {"type": "boolean"}],
                },
            },
        ),
        "press": _obj(
            ["key"],
            {
                "ref": _ref_prop("目标控件引用；缺省为当前焦点控件"),
                "key": {"type": "string", "description": '按键，如 "Enter"、"Escape"、"Tab"、"Shift+Tab"、"Space"'},
            },
        ),
        "scroll": _obj(
            ["ref"],
            {
                "ref": _ref_prop(),
                "direction": {
                    "type": "string",
                    "enum": [d.value for d in UiScrollDirection],
                    "description": "查看方向；缺省为滚动到该控件可见",
                },
            },
        ),
        "read": _obj(
            [],
            {
                "ref": _ref_prop("控件引用；缺省为全部窗口"),
                "maxChars": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": fmt.READ_MAX,
                    "description": f"默认 {fmt.READ_DEFAULT}",
                },
            },
        ),
    }
