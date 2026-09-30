"""由 app-mcp-codegen 生成（target: python），请勿手动修改。
App：示例商城（shop）
总览：演示用购物商城，可管理待办、浏览商品、操作购物车并结算

需要 Python 3.11+（typing.NotRequired）。
"""

from collections.abc import Awaitable, Mapping
from typing import Any, Literal, NotRequired, Protocol, TypedDict, cast

__all__ = ["CatalogSearchCategory", "CatalogSearchParams", "CartAddParams", "CartRemoveItemParams", "CartCheckoutShippingMethod", "CartCheckoutShipping", "CartCheckoutItemsItem", "CartCheckoutParams", "TodosAddParams", "TodosClearParams", "OrdersExportParams", "StatsSummaryPeriod", "StatsSummaryParams", "ShopToolHandlers", "SHOP_TOOL_NAMES", "dispatch_shop_tool"]

# 商品分类
CatalogSearchCategory = Literal["electronics", "home-goods", "food"]

class CatalogSearchParams(TypedDict):
    """按关键词搜索商品，返回商品 ID、名称、价格"""
    keyword: NotRequired[str]
    """商品名或分类，如“耳机”"""
    category: NotRequired[CatalogSearchCategory]
    """商品分类"""
    limit: NotRequired[int]
    """最多返回条数
    取值范围：≥ 1，≤ 100；默认值：20
    """
    inStock: NotRequired[bool]
    """只看有货"""
    maxPrice: NotRequired[float]
    """价格上限（元）
    取值范围：> 0
    """

class CartAddParams(TypedDict):
    """把商品加入购物车"""
    productId: str
    """商品 ID"""
    qty: int
    """数量
    取值范围：≥ 1
    """
    note: NotRequired[str | None]
    """备注，可为 null"""

class CartRemoveItemParams(TypedDict):
    """从购物车移除一个条目"""
    itemId: str

# 配送方式
CartCheckoutShippingMethod = Literal["standard", "express"]

class CartCheckoutShipping(TypedDict):
    """配送选项"""
    method: CartCheckoutShippingMethod
    """配送方式"""
    deliverAfter: NotRequired[str]
    """最早送达时间
    格式：date-time
    """

class CartCheckoutItemsItem(TypedDict):
    itemId: str
    qty: NotRequired[int]
    """取值范围：≥ 1"""

class CartCheckoutParams(TypedDict):
    """提交订单并支付"""
    addressId: str
    """收货地址 ID"""
    coupon: NotRequired[str]
    """优惠码"""
    shipping: NotRequired[CartCheckoutShipping]
    """配送选项"""
    items: NotRequired[list[CartCheckoutItemsItem]]
    """只结算这些条目；省略时结算全部
    元素个数：≥ 1
    """
    giftWrap: NotRequired[bool]
    """默认值：false"""

# 新增一条待办
TodosAddParams = TypedDict(
    "TodosAddParams",
    {
        # 待办内容
        # 长度：1–200
        "title": str,
        # 优先级
        # 可选值：1, 2, 3
        "priority": NotRequired[int],
        # 标签
        "tags": NotRequired[list[str]],
        # 截止日期
        # 格式：date
        "dueDate": NotRequired[str],
        # 分类（属性名是保留字）
        "class": NotRequired[str],
        # 属性名含连字符
        "is-urgent": NotRequired[bool],
    },
)

class TodosClearParams(TypedDict):
    """删除全部已完成的待办"""

class OrdersExportParams(TypedDict):
    """按条件导出订单"""
    filter: NotRequired[Any]
    """筛选条件（任意形式）
    原始 JSON（不支持一般形式的 `oneOf`）
    """
    labels: NotRequired[dict[str, str]]
    """附加标签"""
    extra: NotRequired[Any]
    """透传给导出器的任意 JSON"""
    template: NotRequired[Any]
    """原始 JSON（不支持 `$ref`）"""

# 统计周期
StatsSummaryPeriod = Literal["day", "week", "month"]

class StatsSummaryParams(TypedDict):
    """查看销售概览"""
    period: NotRequired[StatsSummaryPeriod | None]
    """统计周期"""

class ShopToolHandlers(Protocol):
    """示例商城 的工具实现。每个方法接收已由 Host 按 inputSchema 校验过的参数。

    方法可以是普通函数或 async 函数；返回值作为工具结果（JSON）。
    """

    def catalog_search(self, params: CatalogSearchParams) -> Any | Awaitable[Any]:
        """按关键词搜索商品，返回商品 ID、名称、价格

        工具 `catalog.search`「搜索商品」，风险：read
        """
        ...

    def cart_add(self, params: CartAddParams) -> Any | Awaitable[Any]:
        """把商品加入购物车

        工具 `cart.add`「加入购物车」，风险：write
        """
        ...

    def cart_remove_item(self, params: CartRemoveItemParams) -> Any | Awaitable[Any]:
        """从购物车移除一个条目

        工具 `cart.removeItem`「移除购物车条目」，风险：destructive
        """
        ...

    def cart_checkout(self, params: CartCheckoutParams) -> Any | Awaitable[Any]:
        """提交订单并支付

        工具 `cart.checkout`「结算」，风险：payment
        """
        ...

    def todos_add(self, params: TodosAddParams) -> Any | Awaitable[Any]:
        """新增一条待办

        工具 `todos.add`「新增待办」，风险：write
        """
        ...

    def todos_clear(self, params: TodosClearParams) -> Any | Awaitable[Any]:
        """删除全部已完成的待办

        工具 `todos.clear`「清空待办」，风险：destructive
        """
        ...

    def orders_export(self, params: OrdersExportParams) -> Any | Awaitable[Any]:
        """按条件导出订单

        工具 `orders.export`「导出订单」，风险：write
        """
        ...

    def stats_summary(self, params: StatsSummaryParams) -> Any | Awaitable[Any]:
        """查看销售概览

        工具 `stats.summary`「销售概览」，风险：read
        """
        ...

SHOP_TOOL_NAMES: tuple[str, ...] = (
    "catalog.search",
    "cart.add",
    "cart.removeItem",
    "cart.checkout",
    "todos.add",
    "todos.clear",
    "orders.export",
    "stats.summary",
)
"""清单中的全部工具名。"""

def dispatch_shop_tool(handlers: ShopToolHandlers, name: str, arguments: Mapping[str, Any] | None) -> Any:
    """按工具名把调用分派到对应的 handler。参数应已由 Host 按 inputSchema 校验。"""
    args: Any = dict(arguments or {})
    if name == "catalog.search":
        return handlers.catalog_search(cast(CatalogSearchParams, args))
    if name == "cart.add":
        return handlers.cart_add(cast(CartAddParams, args))
    if name == "cart.removeItem":
        return handlers.cart_remove_item(cast(CartRemoveItemParams, args))
    if name == "cart.checkout":
        return handlers.cart_checkout(cast(CartCheckoutParams, args))
    if name == "todos.add":
        return handlers.todos_add(cast(TodosAddParams, args))
    if name == "todos.clear":
        return handlers.todos_clear(cast(TodosClearParams, args))
    if name == "orders.export":
        return handlers.orders_export(cast(OrdersExportParams, args))
    if name == "stats.summary":
        return handlers.stats_summary(cast(StatsSummaryParams, args))
    raise KeyError(f"未知工具：{name}")
