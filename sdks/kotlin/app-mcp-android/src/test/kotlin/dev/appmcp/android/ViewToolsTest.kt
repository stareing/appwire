package dev.appmcp.android

import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.LifecycleRegistry
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/** view 工具启用状态随生命周期切换（不加载原生库，用假的启用 / 注销函数）。 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34])
class ViewToolsTest {
    private class Owner : LifecycleOwner {
        val registry = LifecycleRegistry.createUnsafe(this)
        override val lifecycle: Lifecycle get() = registry
    }

    @Test
    fun enabledOnlyWhileResumedThenDisposed() {
        val owner = Owner()
        val log = mutableListOf<String>()
        owner.registry.currentState = Lifecycle.State.CREATED
        bindEnabled(owner.lifecycle, Lifecycle.State.RESUMED, { log += "enabled=$it" }) { log += "destroy" }
        owner.registry.currentState = Lifecycle.State.STARTED
        owner.registry.currentState = Lifecycle.State.RESUMED
        owner.registry.currentState = Lifecycle.State.STARTED
        owner.registry.currentState = Lifecycle.State.RESUMED
        owner.registry.currentState = Lifecycle.State.DESTROYED
        // 未变化的状态不重复设置；STARTED 仍低于 RESUMED
        assertEquals(listOf("enabled=false", "enabled=true", "enabled=false", "enabled=true", "enabled=false", "destroy"), log)
    }

    @Test
    fun closeStopsObserving() {
        val owner = Owner()
        val log = mutableListOf<Boolean>()
        owner.registry.currentState = Lifecycle.State.RESUMED
        val binding = bindEnabled(owner.lifecycle, Lifecycle.State.STARTED, { log += it }) {}
        binding.close()
        owner.registry.currentState = Lifecycle.State.CREATED
        assertEquals(listOf(true), log)
    }

    @Test
    fun rejectsDestroyedMinState() {
        assertThrows(IllegalArgumentException::class.java) {
            bindEnabled(Owner().lifecycle, Lifecycle.State.DESTROYED, {}) {}
        }
    }
}
