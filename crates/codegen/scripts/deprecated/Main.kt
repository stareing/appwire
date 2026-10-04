// verify.sh 的 deprecated 步骤：实现 deprecated.json 生成的 LegacyToolHandlers 并检查分派与 JSON 往返。
// 覆盖已弃用的成员在 Kotlin 中是 OVERRIDE_DEPRECATION 警告（提示实现方）；实现方按惯例在覆盖处同样标 @Deprecated。
package verify

import appmcp.generated.legacy.CartLegacyClearParams
import appmcp.generated.legacy.LegacyToolHandlers
import appmcp.generated.legacy.LegacyTools
import appmcp.generated.legacy.OrdersFindParams
import appmcp.generated.legacy.OrdersListParams
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

class Impl : LegacyToolHandlers {
    @Deprecated("跟随接口弃用")
    override suspend fun ordersList(params: OrdersListParams): JsonElement =
        LegacyTools.json.encodeToJsonElement(OrdersListParams.serializer(), params)

    override suspend fun ordersFind(params: OrdersFindParams): JsonElement =
        LegacyTools.json.encodeToJsonElement(OrdersFindParams.serializer(), params)

    @Deprecated("跟随接口弃用")
    override suspend fun cartLegacyClear(params: CartLegacyClearParams): JsonElement = JsonPrimitive("cleared")
}

fun check(ok: Boolean, what: String) {
    if (!ok) throw IllegalStateException("检查失败：$what")
}

fun main() = runBlocking {
    val list = LegacyTools.dispatch(Impl(), "orders.list", buildJsonObject { put("status", "paid"); put("page", 2) })
    check(list.toString() == """{"status":"paid","page":2}""", "弃用属性往返：$list")
    val find = LegacyTools.dispatch(
        Impl(),
        "orders.find",
        buildJsonObject {
            put("state", "s")
            put("filter", buildJsonObject { put("legacyTag", "t") })
        },
    )
    check(find.toString() == """{"state":"s","filter":{"legacyTag":"t"}}""", "嵌套弃用属性：$find")
    check(LegacyTools.dispatch(Impl(), "cart.legacyClear", null) == JsonPrimitive("cleared"), "弃用工具分派")
    println("ok")
}
