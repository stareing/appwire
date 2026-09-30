package dev.appmcp

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive

internal val AppMcpJson = Json {
    ignoreUnknownKeys = true
    encodeDefaults = true
}

/**
 * 把 handler 的返回值转换为 JSON。支持：`null`、`Unit`、[JsonElement]、字符串、数字、布尔、
 * `Map<String, *>`、`Iterable`、数组。其他类型请返回 [JsonElement]（或用 [AppMcpRegistrar.typedTool]）。
 */
fun anyToJson(value: Any?): JsonElement = when (value) {
    null, Unit -> JsonNull
    is JsonElement -> value
    is String -> JsonPrimitive(value)
    is Number -> JsonPrimitive(value)
    is Boolean -> JsonPrimitive(value)
    is Enum<*> -> JsonPrimitive(value.name)
    is Map<*, *> -> JsonObject(value.entries.associate { (k, v) -> k.toString() to anyToJson(v) })
    is Iterable<*> -> JsonArray(value.map(::anyToJson))
    is Array<*> -> JsonArray(value.map(::anyToJson))
    is IntArray -> JsonArray(value.map { JsonPrimitive(it) })
    is LongArray -> JsonArray(value.map { JsonPrimitive(it) })
    is DoubleArray -> JsonArray(value.map { JsonPrimitive(it) })
    is BooleanArray -> JsonArray(value.map { JsonPrimitive(it) })
    else -> throw IllegalArgumentException(
        "返回值类型 ${value::class.qualifiedName} 无法转换为 JSON，请返回 JsonElement 或使用 typedTool",
    )
}
