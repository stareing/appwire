package dev.appmcp.compose

import androidx.compose.runtime.AbstractApplier
import androidx.compose.runtime.BroadcastFrameClock
import androidx.compose.runtime.Composition
import androidx.compose.runtime.Recomposer
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshots.Snapshot
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.LifecycleRegistry
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/** view 工具的启用条件 = 所在界面 RESUMED 且 App 侧 `enabled`（弹窗压制）；不加载原生库，只测效果本身。 */
@OptIn(ExperimentalCoroutinesApi::class)
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34])
class ViewEnabledEffectTest {
    private class Owner : LifecycleOwner {
        val registry = LifecycleRegistry.createUnsafe(this)
        override val lifecycle: Lifecycle get() = registry
    }

    private object UnitApplier : AbstractApplier<Unit>(Unit) {
        override fun insertTopDown(index: Int, instance: Unit) = Unit
        override fun insertBottomUp(index: Int, instance: Unit) = Unit
        override fun remove(index: Int, count: Int) = Unit
        override fun move(from: Int, to: Int, count: Int) = Unit
        override fun onClear() = Unit
    }

    /** 驱动一次重组：发出快照变更、等重组器登记帧等待、送一帧、跑完协程。 */
    private fun TestScope.recompose(clock: BroadcastFrameClock) {
        Snapshot.sendApplyNotifications()
        advanceUntilIdle() // 让重组器先登记等待帧
        clock.sendFrame(System.nanoTime())
        advanceUntilIdle()
    }

    @Test
    fun dialogSuppressionSurvivesResume() = runTest {
        val clock = BroadcastFrameClock()
        val recomposer = Recomposer(coroutineContext + clock)
        val job = launch(clock) { recomposer.runRecomposeAndApplyChanges() }
        val owner = Owner().apply { registry.currentState = Lifecycle.State.CREATED }
        val log = mutableListOf<Boolean>()
        var dialogOpen by mutableStateOf(false)

        val composition = Composition(UnitApplier, recomposer)
        composition.setContent {
            ViewEnabledEffect("tool", owner, enabled = !dialogOpen) { log += it }
        }
        advanceUntilIdle()
        owner.registry.currentState = Lifecycle.State.RESUMED
        assertEquals(listOf(true), log)

        // 弹窗打开：下层工具暂停
        dialogOpen = true
        recompose(clock)
        assertEquals(listOf(true, false, false), log)

        // 弹窗仍开着时界面暂停又恢复（如切到后台再回来）：不能被重新启用（回归：之前恢复时总是 setEnabled(true)）
        owner.registry.currentState = Lifecycle.State.STARTED
        owner.registry.currentState = Lifecycle.State.RESUMED
        assertEquals(false, log.last())

        // 弹窗关闭：恢复启用
        dialogOpen = false
        recompose(clock)
        assertEquals(true, log.last())

        // 离开组合：禁用
        composition.dispose()
        assertEquals(false, log.last())
        recomposer.cancel()
        job.cancel()
    }

    @Test
    fun notResumedStaysDisabled() = runTest {
        val clock = BroadcastFrameClock()
        val recomposer = Recomposer(coroutineContext + clock)
        val job = launch(clock) { recomposer.runRecomposeAndApplyChanges() }
        val owner = Owner().apply { registry.currentState = Lifecycle.State.STARTED }
        val log = mutableListOf<Boolean>()
        val composition = Composition(UnitApplier, recomposer)
        composition.setContent { ViewEnabledEffect("tool", owner, enabled = true) { log += it } }
        advanceUntilIdle()
        // STARTED（可见但不在最上层，如被其他 Activity 半遮挡）不启用
        assertEquals(emptyList<Boolean>(), log)
        owner.registry.currentState = Lifecycle.State.RESUMED
        owner.registry.currentState = Lifecycle.State.STARTED
        assertEquals(listOf(true, false), log)
        composition.dispose()
        recomposer.cancel()
        job.cancel()
    }
}
