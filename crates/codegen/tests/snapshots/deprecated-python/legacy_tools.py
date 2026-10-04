"""由 app-mcp-codegen 生成（target: python），请勿手动修改。
App：弃用示例（legacy）
简介：演示工具级与参数级弃用的生成结果

需要 Python 3.11+（typing.NotRequired）。
"""

from collections.abc import Awaitable, Mapping
from typing import Any, Literal, NotRequired, Protocol, TypedDict, cast

__all__ = ["OrdersListParams", "OrdersFindFilter", "OrdersFindParams", "CartLegacyClearParams", "LegacyToolHandlers", "LEGACY_TOOL_NAMES", "dispatch_legacy_tool"]

class OrdersListParams(TypedDict):
    """按状态列出订单（旧版）"""
    status: str
    """订单状态
    参数已弃用（inputSchema 中 deprecated: true）
    """
    page: NotRequired[int]
    """页码
    取值范围：≥ 1
    参数已弃用（inputSchema 中 deprecated: true）
    """
    limit: NotRequired[int]
    """最多返回条数"""

class OrdersFindFilter(TypedDict):
    """过滤条件"""
    keyword: NotRequired[str]
    """关键词"""
    legacyTag: NotRequired[str]
    """旧标签
    参数已弃用（inputSchema 中 deprecated: true）
    """

class OrdersFindParams(TypedDict):
    """按状态查找订单"""
    state: str
    """订单状态"""
    cursor: NotRequired[str]
    """分页游标"""
    filter: NotRequired[OrdersFindFilter]
    """过滤条件"""

class CartLegacyClearParams(TypedDict):
    """清空购物车（旧版）"""

class LegacyToolHandlers(Protocol):
    """弃用示例 的工具实现。每个方法接收已由 Host 按 inputSchema 校验过的参数。

    方法可以是普通函数或 async 函数；返回值作为工具结果（JSON）。
    """

    def orders_list(self, params: OrdersListParams) -> Any | Awaitable[Any]:
        """按状态列出订单（旧版）

        工具 `orders.list`「列出订单」，风险：read
        已弃用：旧版 "列表" 接口 */ 不再维护：$x ${y} \\(z) 'q' <b>&amp; #{w}
        请改用分页更好的新版（改用工具 orders.find；计划于 2027-06-30 移除）
        """
        ...

    def orders_find(self, params: OrdersFindParams) -> Any | Awaitable[Any]:
        """按状态查找订单

        工具 `orders.find`「查找订单」，风险：read
        """
        ...

    def cart_legacy_clear(self, params: CartLegacyClearParams) -> Any | Awaitable[Any]:
        """清空购物车（旧版）

        工具 `cart.legacyClear`「清空购物车」，风险：write
        已弃用：改用 cart.clear
        """
        ...

LEGACY_TOOL_NAMES: tuple[str, ...] = (
    "orders.list",
    "orders.find",
    "cart.legacyClear",
)
"""清单中的全部工具名。"""

def dispatch_legacy_tool(handlers: LegacyToolHandlers, name: str, arguments: Mapping[str, Any] | None) -> Any:
    """按工具名把调用分派到对应的 handler。参数应已由 Host 按 inputSchema 校验。"""
    args: Any = dict(arguments or {})
    if name == "orders.list":
        return handlers.orders_list(cast(OrdersListParams, args))
    if name == "orders.find":
        return handlers.orders_find(cast(OrdersFindParams, args))
    if name == "cart.legacyClear":
        return handlers.cart_legacy_clear(cast(CartLegacyClearParams, args))
    raise KeyError(f"未知工具：{name}")
