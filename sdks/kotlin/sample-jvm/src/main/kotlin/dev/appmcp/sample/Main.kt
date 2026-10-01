package dev.appmcp.sample

import dev.appmcp.AppMcp
import dev.appmcp.AppMcpConfig
import dev.appmcp.AppOverview
import dev.appmcp.ErrorKind
import dev.appmcp.Risk
import dev.appmcp.ToolCallException
import dev.appmcp.ToolResult
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.int
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonObject

@Serializable
data class AddItem(val sku: String, val qty: Int = 1)

@Serializable
data class Cart(val items: Map<String, Int>)

/** 控制台示例：`./gradlew :sample-jvm:run --args="ws://127.0.0.1:7717/app"`。 */
fun main(args: Array<String>): Unit = runBlocking {
    val cart = java.util.concurrent.ConcurrentHashMap<String, Int>()
    val client = AppMcp.create(
        AppMcpConfig(
            appId = "sample-jvm",
            appName = "Kotlin 示例",
            hostUrl = args.firstOrNull(),
            overview = AppOverview(summary = "示例购物车：加入商品、查看购物车", body = "用于演示 Kotlin SDK。"),
            onPaired = { token -> println("配对成功，token = $token（应持久化）") },
            onLog = { level, msg -> System.err.println("[$level] $msg") },
        ),
    )

    val counterSchema = buildJsonObject {
        put("type", "object")
        putJsonObject("properties") { putJsonObject("n") { put("type", "integer") } }
    }
    client.tool("counter.double", "把数字乘以 2", counterSchema, risk = Risk.READ) { a, _ ->
        val n = a["n"]?.jsonPrimitive?.int ?: throw ToolCallException(ErrorKind.INVALID_INPUT, "缺少 n")
        delay(10)
        JsonPrimitive(n * 2)
    }
    client.typedTool<AddItem, Cart>("cart.add", "加入购物车") { item, ctx ->
        cart.merge(item.sku, item.qty, Int::plus)
        ctx.addStateHint("cart")
        Cart(cart.toMap())
    }
    client.tool("cart.clear", "清空购物车", risk = Risk.DESTRUCTIVE) { _, _ ->
        cart.clear()
        ToolResult(null, stateHints = listOf("cart"))
    }
    client.resource("cart", "当前购物车") { cart.toMap() }

    client.use {
        it.start()
        println("instanceId = ${it.instanceId}，按 Ctrl+C 退出")
        it.state.collect { s -> println("状态：${s.status} ${s.reason ?: ""}") }
    }
}
