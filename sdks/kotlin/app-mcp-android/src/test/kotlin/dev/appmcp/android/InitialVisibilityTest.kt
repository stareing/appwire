package dev.appmcp.android

import dev.appmcp.Visibility
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/** 后台冷启动（没有 Activity 处于 started）时客户端以隐藏创建（spec/lifecycle.md 第 13 节 B4「后台连接」）。 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34])
class InitialVisibilityTest {
    @Test
    fun hiddenWhenCreatedWithoutForegroundActivity() {
        assertEquals(Visibility.HIDDEN, AppMcpAndroid.initialVisibility())
    }

    @Test
    fun unknownOffMainThread() {
        var result: Visibility? = Visibility.VISIBLE
        val t = Thread { result = AppMcpAndroid.initialVisibility() }
        t.start()
        t.join()
        assertNull("非主线程无法判断前后台，保持核心默认", result)
    }
}
