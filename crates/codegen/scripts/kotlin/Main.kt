// verify.sh 使用：实现 kotlin target 生成的 ShopToolHandlers 并检查分派与 JSON 往返。
package verify

import appmcp.generated.shop.CartAddParams
import appmcp.generated.shop.CartCheckoutParams
import appmcp.generated.shop.CartRemoveItemParams
import appmcp.generated.shop.CatalogSearchParams
import appmcp.generated.shop.OrdersExportParams
import appmcp.generated.shop.ShopToolHandlers
import appmcp.generated.shop.ShopTools
import appmcp.generated.shop.StatsSummaryParams
import appmcp.generated.shop.TodosAddParams
import appmcp.generated.shop.TodosClearParams
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonPrimitive

class Impl : ShopToolHandlers {
    override suspend fun catalogSearch(params: CatalogSearchParams): JsonElement =
        ShopTools.json.encodeToJsonElement(CatalogSearchParams.serializer(), params)

    override suspend fun cartAdd(params: CartAddParams): JsonElement =
        JsonPrimitive("${params.productId} x ${params.qty}")

    override suspend fun cartRemoveItem(params: CartRemoveItemParams): JsonElement = JsonPrimitive(params.itemId)

    override suspend fun cartCheckout(params: CartCheckoutParams): JsonElement =
        ShopTools.json.encodeToJsonElement(CartCheckoutParams.serializer(), params)

    override suspend fun todosAdd(params: TodosAddParams): JsonElement =
        ShopTools.json.encodeToJsonElement(TodosAddParams.serializer(), params)

    override suspend fun todosClear(params: TodosClearParams): JsonElement = JsonNull

    override suspend fun ordersExport(params: OrdersExportParams): JsonElement =
        ShopTools.json.encodeToJsonElement(OrdersExportParams.serializer(), params)

    override suspend fun statsSummary(params: StatsSummaryParams): JsonElement =
        JsonPrimitive(params.period?.name ?: "ALL")
}

private fun parse(text: String): JsonElement = ShopTools.json.parseToJsonElement(text)

fun main() = runBlocking {
    val h = Impl()
    val checkout = """{"addressId":"a1","shipping":{"method":"express"},"items":[{"itemId":"i1","qty":2}],"giftWrap":true}"""
    val out = ShopTools.dispatch(h, "cart.checkout", parse(checkout))
    check(out.toString() == checkout) { "cart.checkout：$out" }
    val todo = ShopTools.dispatch(h, "todos.add", parse("""{"title":"t","class":"c","is-urgent":true}"""))
    check(todo.toString() == """{"title":"t","class":"c","is-urgent":true}""") { "todos.add：$todo" }
    val period = ShopTools.dispatch(h, "stats.summary", parse("""{"period":"week"}"""))
    check(period == JsonPrimitive("WEEK")) { "stats.summary：$period" }
    val export = ShopTools.dispatch(h, "orders.export", parse("""{"labels":{"a":"b"},"extra":[1,null]}"""))
    check(export.toString() == """{"labels":{"a":"b"},"extra":[1,null]}""") { "orders.export：$export" }
    check(ShopTools.dispatch(h, "todos.clear", null) == JsonNull)
    val failed = runCatching { ShopTools.dispatch(h, "cart.add", parse("""{"qty":1}""")) }
    check(failed.isFailure) { "缺少必填字段应失败" }
    println("ok")
}
