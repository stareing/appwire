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
    include(":app-mcp-android")
    include(":app-mcp-hub-android")
    // 可选：Jetpack Compose / Navigation Compose 绑定（view 工具可见性、导航适配；第 4c 项 E）
    include(":app-mcp-compose")
    // 最小 Android 示例 App（真机验证用）
    include(":sample-android")
} else {
    logger.warn("未找到 Android SDK，跳过 Android 模块")
}
