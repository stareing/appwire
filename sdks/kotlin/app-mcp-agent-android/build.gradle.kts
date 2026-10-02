// Android 库：Agent 侧客户端（TASKS 4g f）。绑定独立 Hub App（Intent 动作 `dev.appmcp.HUB`），Binder 上换得一个
// socketpair fd，在其上跑最小 MCP 会话（initialize / tools/list / tools/call；每行一条 JSON-RPC）。
// 不含原生库：Agent App 不需要嵌入 Hub。
plugins {
    id("com.android.library")
    kotlin("android")
}

android {
    namespace = "dev.appmcp.agent"
    compileSdk = 35

    defaultConfig {
        minSdk = 24
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
    api(project(":app-mcp-binder"))
    api("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.10.2")
    api("org.jetbrains.kotlinx:kotlinx-serialization-json:1.9.0")

    testImplementation("junit:junit:4.13.2")
    testImplementation("org.jetbrains.kotlinx:kotlinx-coroutines-test:1.10.2")
}
