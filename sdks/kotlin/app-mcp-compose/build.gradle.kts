// 可选 Android 库：Jetpack Compose 绑定（第 4c 项 E，spec/protocol.md 3.4）。
//   · ViewToolEffect / rememberViewTool：view 工具只在所在界面 RESUMED（可见且在最上层）时启用（LifecycleResumeEffect）
//   · navigationRouter / NavigationHandlerEffect：Host 的 app/navigate → NavController.navigate(route)
//   · ComposeUiExpander / Modifier.mcpDeclared：进程内控件兜底读 Compose 语义树（第 4c 项 H，spec/ui-fallback.md）
// 单独成模块：不用 Compose 的 App 不引入 Compose / Navigation 依赖（:app-mcp-android 只依赖 lifecycle）。
plugins {
    id("com.android.library")
    kotlin("android")
    kotlin("plugin.compose")
}

android {
    namespace = "dev.appmcp.compose"
    compileSdk = 35

    defaultConfig {
        minSdk = 24
    }

    buildFeatures {
        compose = true
    }

    testOptions {
        unitTests.isIncludeAndroidResources = true
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

kotlin {
    compilerOptions {
        jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17)
    }
}

dependencies {
    api(project(":app-mcp-android"))
    api("androidx.compose.runtime:runtime:1.9.4")
    api("androidx.lifecycle:lifecycle-runtime-compose:2.9.4")
    api("androidx.navigation:navigation-compose:2.9.5")
    // 控件兜底：SemanticsOwner / 语义键（ComposeUiExpander）
    api("androidx.compose.ui:ui:1.9.0")

    testImplementation("junit:junit:4.13.2")
    testImplementation("org.robolectric:robolectric:4.14.1")
    testImplementation("androidx.test:core:1.6.1")
    testImplementation("org.jetbrains.kotlinx:kotlinx-coroutines-test:1.10.2")
    // 控件兜底测试：在 Robolectric 的 Activity 中渲染真实 Compose 界面（未引入 compose ui-test）
    testImplementation("androidx.activity:activity-compose:1.8.0")
    testImplementation("androidx.compose.foundation:foundation:1.9.0")
}
