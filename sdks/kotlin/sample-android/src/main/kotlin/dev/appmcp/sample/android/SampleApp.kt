package dev.appmcp.sample.android

import android.app.Application
import android.content.Intent
import android.os.SystemClock
import android.util.Log
import dev.appmcp.AppMcp
import dev.appmcp.AppMcpConfig
import dev.appmcp.AppOverview
import dev.appmcp.ErrorKind
import dev.appmcp.LogLevel
import dev.appmcp.NavigateFunction
import dev.appmcp.NavigationResult
import dev.appmcp.Risk
import dev.appmcp.ToolCallException
import dev.appmcp.ToolAnnotations
import dev.appmcp.ToolResult
import dev.appmcp.UserActionReason
import dev.appmcp.android.AppMcpAndroid
import dev.appmcp.android.AppMcpProvider
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.filterNotNull
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.intOrNull
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonObject

internal const val TAG = "AppMcpSample"

/** 导航时把界面切到前台的等待上限：超时 = 系统没让界面到前台（如后台启动 Activity 受限）。 */
private const val FOREGROUND_TIMEOUT_MS = 5_000L

/**
 * 客户端放在 Application 里（而不是 Activity）：进程被唤醒广播冷启动时没有 Activity，
 * WakeWorker 通过 [AppMcpProvider] 取得客户端。
 *
 * 生命周期演示（Android 默认 on-demand + sleepOnBackground）：启动不连接，进入前台时连上；没有调用 10 s
 * （graceMs）后休眠，调用后按 Host 租约 + 2 s 合并窗口休眠；切到后台空闲即休眠。休眠后 Host 用显式广播唤醒：
 *   adb shell am broadcast -a dev.appmcp.action.WAKE \
 *     -n dev.appmcp.sample.android/dev.appmcp.android.WakeReceiver --es token <t>
 */
class SampleApp : Application(), AppMcpProvider {
    val counter = MutableStateFlow(0)
    val notes = MutableStateFlow<List<String>>(emptyList())

    /**
     * 处于前台（RESUMED）的 Compose 导航界面登记的导航函数（[NavActivity]）；null = 界面不在前台。
     * @invariant 只在主线程写（Activity 生命周期回调）
     */
    val foregroundNavigator = MutableStateFlow<NavigateFunction?>(null)
    private val appScope = CoroutineScope(SupervisorJob() + Dispatchers.Main)

    val mcp: AppMcp by lazy { createClient() }

    override fun appMcp(): AppMcp = mcp

    override fun onCreate() {
        super.onCreate()
        mcp // 冷启动（包括被唤醒广播拉起）时立即创建并启动（on-demand：前台或唤醒时才连接）
    }

    private fun createClient(): AppMcp {
        val c = AppMcpAndroid.create(
            this,
            AppMcpConfig(
                appId = "sample-android", // 须匹配 [a-z][a-z0-9-]{0,62}，不能直接用包名
                appName = "app-mcp Android 示例",
                hostUrl = "ws://127.0.0.1:7717/app", // 配合 adb reverse tcp:7717 tcp:7717
                appVersion = "0.1.0",
                overview = AppOverview(summary = "Android 示例：回显文本、计数器", body = null),
                onPaired = { token -> Log.i(TAG, "配对成功 token=$token") },
                onLog = { level, msg -> Log.println(priority(level), TAG, "[native] $msg") },
                onIdleExit = { Log.i(TAG, "onIdleExit（residency=keep 时不会出现）") },
            ),
        )

        // 导航回调在 start() 之前设置：握手时声明 capabilities.navigate（被唤醒冷启动、还没有界面时也能导航）。
        c.setNavigationHandler(::navigate)

        val echoSchema = buildJsonObject {
            put("type", "object")
            putJsonObject("properties") { putJsonObject("text") { put("type", "string") } }
            put("required", buildJsonArray { add(JsonPrimitive("text")) })
        }
        c.tool("demo.echo", "原样返回 text，并附带执行线程名", echoSchema, risk = Risk.READ) { args, _ ->
            val t = args["text"]?.jsonPrimitive?.contentOrNull
                ?: throw ToolCallException(ErrorKind.INVALID_INPUT, "缺少 text")
            Log.i(TAG, "demo.echo text=$t thread=${Thread.currentThread().name}")
            buildJsonObject {
                put("text", t)
                put("thread", Thread.currentThread().name)
            }
        }

        val incSchema = buildJsonObject {
            put("type", "object")
            putJsonObject("properties") { putJsonObject("by") { put("type", "integer") } }
        }
        c.tool("demo.counter.increment", "计数器加 by（默认 1，须为正），返回新值", incSchema) { args, _ ->
            val by = args["by"]?.jsonPrimitive?.intOrNull ?: 1
            if (by <= 0) {
                throw ToolCallException(
                    ErrorKind.INVALID_INPUT, "by 必须为正",
                    buildJsonObject { put("field", "by"); put("min", 1) },
                )
            }
            counter.value += by
            Log.i(TAG, "demo.counter.increment by=$by -> ${counter.value}")
            ToolResult(
                JsonPrimitive(counter.value),
                stateHints = listOf("demo.counter"),
                summary = "计数器已加 $by，当前为 ${counter.value}",
            )
        }

        // 无返回值：Hub 对模型输出"已完成"；破坏性注解原样转给 Agent。
        c.tool(
            "demo.counter.reset", "计数器清零（无返回值）",
            annotations = ToolAnnotations(destructiveHint = true, idempotentHint = true),
        ) { _, ctx ->
            Log.i(TAG, "demo.counter.reset")
            counter.value = 0
            ctx.addStateHint("demo.counter")
        }

        val stepsSchema = buildJsonObject {
            put("type", "object")
            putJsonObject("properties") { putJsonObject("steps") { put("type", "integer") } }
        }
        // 每秒报告一次进度；取消时 handler 协程被取消，日志记录取消原因。
        c.tool(
            "demo.long_task", "模拟耗时任务：每秒一步（steps 默认 5），报告进度，可取消", stepsSchema,
            annotations = ToolAnnotations(readOnlyHint = true),
        ) { args, ctx ->
            val steps = args["steps"]?.jsonPrimitive?.intOrNull?.coerceIn(1, 60) ?: 5
            try {
                for (i in 1..steps) {
                    delay(1_000)
                    ctx.progress(i.toDouble(), steps.toDouble(), "第 $i/$steps 步")
                }
            } finally {
                if (ctx.isCancelled) Log.i(TAG, "demo.long_task cancelled reason=${ctx.cancelReason}")
            }
            Log.i(TAG, "demo.long_task done steps=$steps")
            ToolResult(JsonPrimitive(steps), summary = "完成 $steps 步")
        }

        c.tool("demo.account.profile", "读取账户资料（示例：总是要求先登录）", risk = Risk.READ) { _, _ ->
            throw ToolCallException.userActionRequired(
                "登录已过期，请在 App 内重新登录后重试", UserActionReason.LOGIN, "appmcp-sample://login",
            )
        }

        c.tool("demo.sync", "模拟后台同步：立即返回，后台继续 3 秒（期间持有、不休眠）") { _, ctx ->
            val hold = ctx.hold()
            appScope.launch {
                delay(3_000)
                hold.close()
                Log.i(TAG, "demo.sync 后台任务结束，释放持有")
            }
            "started"
        }

        c.resource("demo.counter", "当前计数", "application/json") {
            buildJsonObject {
                put("value", counter.value)
                put("device", android.os.Build.MODEL)
            }
        }
        c.start()
        Log.i(TAG, "started instanceId=${c.instanceId} toolsHash=${c.toolsHash}")
        return c
    }

    /**
     * Host 的 `app/navigate`：界面在前台时直接交给它；否则先把 [NavActivity] 切到前台（冷启动或从后台带回），
     * 等它登记导航函数后再导航。
     * @error 界面没能在 [FOREGROUND_TIMEOUT_MS] 内到前台 → [NavigationResult.Failed]
     */
    private suspend fun navigate(page: String, params: JsonObject?): NavigationResult {
        foregroundNavigator.value?.let { return it(page, params) }
        val t0 = SystemClock.elapsedRealtime()
        startActivity(
            Intent(this, NavActivity::class.java)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_REORDER_TO_FRONT),
        )
        val navigator = withTimeoutOrNull(FOREGROUND_TIMEOUT_MS) { foregroundNavigator.filterNotNull().first() }
        val dt = SystemClock.elapsedRealtime() - t0
        if (navigator == null) {
            Log.w(TAG, "navigate page=$page：界面 ${dt}ms 内未到前台")
            return NavigationResult.Failed("无法把界面切到前台（系统可能限制后台启动界面）")
        }
        Log.i(TAG, "navigate page=$page：界面到前台用时 ${dt}ms")
        return navigator(page, params)
    }

    private fun priority(level: LogLevel): Int = when (level) {
        LogLevel.ERROR -> Log.ERROR
        LogLevel.WARN -> Log.WARN
        LogLevel.INFO -> Log.INFO
        LogLevel.DEBUG -> Log.DEBUG
    }
}
