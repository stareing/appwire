"""verify.sh 的 deprecated 步骤：实现 deprecated.json 生成的 LegacyToolHandlers（含已弃用的方法）并分派。"""

from typing import Any

from legacy_tools import LegacyToolHandlers, OrdersFindParams, OrdersListParams, dispatch_legacy_tool


class Impl:
    def orders_list(self, params: OrdersListParams) -> Any:
        return params["status"], params.get("page")

    def orders_find(self, params: OrdersFindParams) -> Any:
        return params["state"], params.get("filter", {}).get("legacyTag")

    def cart_legacy_clear(self, params: Any) -> Any:
        return "cleared"


handlers: LegacyToolHandlers = Impl()
assert dispatch_legacy_tool(handlers, "orders.list", {"status": "paid", "page": 2}) == ("paid", 2)
assert dispatch_legacy_tool(handlers, "orders.find", {"state": "s", "filter": {"legacyTag": "t"}}) == ("s", "t")
assert dispatch_legacy_tool(handlers, "cart.legacyClear", None) == "cleared"
print("ok")
