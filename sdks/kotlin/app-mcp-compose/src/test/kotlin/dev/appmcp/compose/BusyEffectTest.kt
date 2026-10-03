package dev.appmcp.compose

import androidx.compose.runtime.AbstractApplier
import androidx.compose.runtime.BroadcastFrameClock
import androidx.compose.runtime.Composition
import androidx.compose.runtime.Recomposer
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshots.Snapshot
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

/** BusyEffect：active 时持有一段忙碌作用域，变为 false / 离开组合时归还；不加载原生库，只测效果本身。 */
@OptIn(ExperimentalCoroutinesApi::class)
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34])
class BusyEffectTest {
    private object UnitApplier : AbstractApplier<Unit>(Unit) {
        override fun insertTopDown(index: Int, instance: Unit) = Unit
        override fun insertBottomUp(index: Int, instance: Unit) = Unit
        override fun remove(index: Int, count: Int) = Unit
        override fun move(from: Int, to: Int, count: Int) = Unit
        override fun onClear() = Unit
    }

    private fun TestScope.recompose(clock: BroadcastFrameClock) {
        Snapshot.sendApplyNotifications()
        advanceUntilIdle()
        clock.sendFrame(System.nanoTime())
        advanceUntilIdle()
    }

    @Test
    fun holdsScopeWhileActive() = runTest {
        val clock = BroadcastFrameClock()
        val recomposer = Recomposer(coroutineContext + clock)
        val job = launch(clock) { recomposer.runRecomposeAndApplyChanges() }
        var held = 0
        var begun = 0
        val begin = { begun += 1; held += 1; AutoCloseable { held -= 1 } }
        var editing by mutableStateOf(true)
        var dragging by mutableStateOf(false)

        val composition = Composition(UnitApplier, recomposer)
        composition.setContent {
            BusyScopeEffect("client", editing, begin)
            BusyScopeEffect("client", dragging, begin)
        }
        advanceUntilIdle()
        assertEquals(1, held)

        // 两处同时忙碌：各持一份
        dragging = true
        recompose(clock)
        assertEquals(2, held)

        // 一处结束只归还自己那份
        editing = false
        recompose(clock)
        assertEquals(1, held)

        // 无关重组不得重复开始
        val before = begun
        recompose(clock)
        assertEquals(before, begun)

        // 离开组合：全部归还
        composition.dispose()
        assertEquals(0, held)
        recomposer.cancel()
        job.cancel()
    }
}
