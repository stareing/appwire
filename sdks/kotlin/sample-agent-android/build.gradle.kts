// 最小示例 Agent（TASKS 4g f）：绑定独立 Hub App（:hub-app-android），MCP 会话 initialize → tools/list → tools/call，
// 结果显示在界面并写入 Logcat（标签 AppMcpAgent）。不含原生库。
//   adb shell am start -n dev.appmcp.sample.agent/.AgentActivity \
//     --es tool sample-android.demo.echo --es args '{"text":"hi"}'
plugins {
    id("com.android.application")
    kotlin("android")
}

android {
    namespace = "dev.appmcp.sample.agent"
    compileSdk = 35

    defaultConfig {
        applicationId = "dev.appmcp.sample.agent"
        minSdk = 24
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0"
    }

    buildTypes {
        release {
            signingConfig = signingConfigs.getByName("debug")
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"))
        }
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
    implementation(project(":app-mcp-agent-android"))
}
