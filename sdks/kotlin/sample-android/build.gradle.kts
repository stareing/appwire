// 最小 Android 示例 App：连接 Host（ws://127.0.0.1:7717，配合 `adb reverse tcp:7717 tcp:7717`），
// 注册 demo.echo、demo.counter.increment、demo.counter.reset（无返回值）、demo.long_task（进度 / 取消）、
// demo.account.profile（USER_ACTION_REQUIRED）、demo.sync 工具与 demo.counter 资源；生命周期为 idle 模式
// （空闲 10 s / 后台 5 s 休眠），可被 `am broadcast -a dev.appmcp.action.WAKE -n <pkg>/dev.appmcp.android.WakeReceiver
// --es token <t>` 唤醒。另带进程内 Hub 自检（HubSelfTest，`--ez hubSelfTest true` 启动）。
// Compose 导航示例（NavActivity，第 4c 项）：counter / notes 两个页面的 view 工具、Navigation Compose 导航回调、
// 一个确认弹窗（打开时压制下层工具）。
// 可调试构建另开启进程内控件兜底（ui.outline / click / fill / press / scroll / read，第 4c 项 H），Compose 按钮用
// Modifier.mcpDeclared 标出已声明的工具。
//
// 发布形态：release 开启 R8（minify + 资源压缩），规则全部来自依赖（jar 的 META-INF/proguard、AAR 的 consumer 规则）；
// 按 ABI 拆 APK（arm64-v8a、armeabi-v7a、x86_64、x86 + universal），App Bundle 按 ABI 拆分发。
//   ./gradlew :sample-android:assembleRelease                         → build/outputs/apk/release/*-<abi>-release.apk
//   ./gradlew :sample-android:assembleRelease -Pappmcp.abis=arm64-v8a  （只出指定 ABI 的拆分包，真机调试省时间）
//   ./gradlew :sample-android:bundleRelease                           → build/outputs/bundle/release/*.aab
// release 用 debug keystore 签名，便于直接安装到真机。
plugins {
    id("com.android.application")
    kotlin("android")
    kotlin("plugin.compose")
}

// SDK 原生库提供的 ABI（bindings/*/scripts/generate.sh --android）；-Pappmcp.abis=arm64-v8a 只出指定 ABI。
val allAbis = listOf("arm64-v8a", "armeabi-v7a", "x86_64", "x86")
val abis = (findProperty("appmcp.abis") as String?)
    ?.split(",")?.map { it.trim() }?.filter { it.isNotEmpty() }
    ?: allAbis

android {
    namespace = "dev.appmcp.sample.android"
    compileSdk = 35

    defaultConfig {
        applicationId = "dev.appmcp.sample.android"
        minSdk = 24
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0"
    }

    splits {
        abi {
            // App Bundle 自己按 ABI 拆分（见 bundle {}）；AGP 不允许 bundle 与多 APK 拆分同时开启
            // （shrinkResources 时报 "Multiple shrunk-resources files"，issuetracker 402800800）。
            isEnable = gradle.startParameter.taskNames.none { it.contains("bundle", ignoreCase = true) }
            reset()
            include(*abis.toTypedArray())
            // 全部 ABI 时另出一个 universal APK（侧载 / 不确定设备 ABI 时用）
            isUniversalApk = abis.size == allAbis.size
        }
    }

    bundle {
        abi { enableSplit = true }
    }

    packaging {
        jniLibs {
            // JNA 的 AAR 还带 armeabi / mips / mips64 的 libjnidispatch.so，SDK 的 Rust 库没有这些 ABI；
            // 不排除的话 universal APK / App Bundle 会出现缺 libapp_mcp_*.so 的 ABI 目录。
            excludes += listOf("lib/armeabi/**", "lib/mips/**", "lib/mips64/**")
        }
    }

    buildTypes {
        release {
            // 示例 App：release 用 debug keystore 签名，便于直接安装到真机。
            signingConfig = signingConfigs.getByName("debug")
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
        }
    }

    buildFeatures {
        compose = true
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
    // 进程内 Hub 自检（HubSelfTest）
    implementation(project(":app-mcp-hub-android"))
    // Compose 导航示例（NavActivity）：view 工具与导航适配；界面只用 foundation（不引入 Material）
    implementation(project(":app-mcp-compose"))
    implementation("androidx.activity:activity-compose:1.8.0")
    implementation("androidx.compose.foundation:foundation:1.9.0")
}
