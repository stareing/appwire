package dev.appmcp.sample.android

import android.app.Activity
import android.os.Bundle
import android.util.Log
import android.widget.Button
import android.widget.LinearLayout
import android.widget.TextView
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.launch

/** 显示连接状态、计数与 toolsHash；按钮演示 App 主动 wake / sleep。客户端归 [SampleApp] 所有。
 * 带 `hubSelfTest=true` extra 启动时另跑一次进程内 Hub 自检（[HubSelfTest]）。 */
class MainActivity : Activity() {
    private val uiScope = CoroutineScope(SupervisorJob() + Dispatchers.Main)

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val app = application as SampleApp
        val client = app.mcp
        val label = TextView(this).apply { textSize = 20f; setPadding(48, 96, 48, 48) }
        val wake = Button(this).apply { text = "立即回连（wake）"; setOnClickListener { client.wake() } }
        val sleep = Button(this).apply { text = "立即休眠（sleep）"; setOnClickListener { client.sleep() } }
        setContentView(
            LinearLayout(this).apply {
                orientation = LinearLayout.VERTICAL
                addView(label)
                addView(wake)
                addView(sleep)
            },
        )
        uiScope.launch {
            client.state.combine(app.counter) { s, n -> s to n }.collect { (s, n) ->
                val status = "${s.status}${s.reason?.let { " ($it)" } ?: ""}"
                Log.i(TAG, "state=$status")
                label.text = "app-mcp 示例（on-demand：前台连接 / 无调用 10 s 或进入后台休眠）\n\n" +
                    "状态：$status\n计数：$n\ntoolsHash：${client.toolsHash}"
            }
        }
        // 进程内 Hub 自检（am start … --ez hubSelfTest true），见 HubSelfTest。
        if (intent.getBooleanExtra("hubSelfTest", false)) uiScope.launch { HubSelfTest.run() }
    }

    override fun onDestroy() {
        uiScope.cancel()
        super.onDestroy()
    }
}
