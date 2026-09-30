// 最小 Android 示例 App：连接 Host（ws://127.0.0.1:7717，配合 `adb reverse tcp:7717 tcp:7717`），
// 注册 demo.echo、demo.counter.increment、demo.sync 三个工具与 demo.counter 资源；生命周期为 idle 模式
// （空闲 10 s / 后台 5 s 休眠），可被 `am broadcast -a dev.appmcp.action.WAKE -n <pkg>/dev.appmcp.android.WakeReceiver
// --es token <t>` 唤醒。
//   ./gradlew :sample-android:assembleRelease -Pappmcp.abis=arm64-v8a   （release 用 debug keystore 签名）
plugins {
    id("com.android.application")
    kotlin("android")
}

android {
    namespace = "dev.appmcp.sample.android"
    compileSdk = 35

    defaultConfig {
        applicationId = "dev.appmcp.sample.android"
        minSdk = 24
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0"
        // 可选：-Pappmcp.abis=arm64-v8a 只打包指定 ABI（真机调试时减小 APK）
        (findProperty("appmcp.abis") as String?)?.let { abis ->
            ndk { abiFilters += abis.split(",").map { it.trim() }.filter { it.isNotEmpty() } }
        }
    }

    buildTypes {
        release {
            // 示例 App：release 用 debug keystore 签名，便于直接安装到真机。
            signingConfig = signingConfigs.getByName("debug")
            isMinifyEnabled = false
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
    implementation(project(":app-mcp-android"))
}
