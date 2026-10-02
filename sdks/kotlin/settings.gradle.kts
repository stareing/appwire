pluginManagement {
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}

dependencyResolutionManagement {
    repositories {
        google()
        mavenCentral()
    }
}

rootProject.name = "app-mcp-kotlin"

include(":app-mcp")
include(":app-mcp-hub")
include(":sample-jvm")

// Android 模块需要 Android SDK（ANDROID_HOME / local.properties 的 sdk.dir）；
// 没有时跳过，JVM 模块照常构建。
val androidSdk = System.getenv("ANDROID_HOME")
    ?: System.getenv("ANDROID_SDK_ROOT")
    ?: file("local.properties").takeIf { it.exists() }?.readLines()
        ?.firstOrNull { it.startsWith("sdk.dir=") }?.substringAfter("sdk.dir=")
    ?: "${System.getProperty("user.home")}/Android/Sdk".takeIf { file(it).isDirectory }
if (androidSdk != null && file(androidSdk).isDirectory) {
    // Binder 上交换 fd 的共用实现（按名寻址：App 端 ToolsService、Hub 端拨号、独立 Hub App、Agent 客户端）
    include(":app-mcp-binder")
    include(":app-mcp-android")
    include(":app-mcp-hub-android")
    // 可选：Jetpack Compose / Navigation Compose 绑定（view 工具可见性、导航适配；第 4c 项 E）
    include(":app-mcp-compose")
    // 最小 Android 示例 App（真机验证用）
    include(":sample-android")
    // 独立 Hub App 原型与 Agent 侧客户端（TASKS 4g d / f）
    include(":hub-app-android")
    include(":app-mcp-agent-android")
    include(":sample-agent-android")
} else {
    logger.warn("未找到 Android SDK，跳过 Android 模块")
}
