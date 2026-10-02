// Android 库：复用 :app-mcp 的代码，打包各 ABI 的 libapp_mcp_uniffi.so（src/main/jniLibs/<abi>/），
// 并提供 Dispatchers.Main 与生命周期辅助。.so 由
//   bash bindings/uniffi/scripts/generate.sh --android
// 交叉编译生成。
plugins {
    id("com.android.library")
    kotlin("android")
}

android {
    namespace = "dev.appmcp.android"
    compileSdk = 35

    defaultConfig {
        minSdk = 24
        consumerProguardFiles("consumer-rules.pro")
    }

    testOptions {
        // Robolectric 需要合并后的 manifest（WakeReceiver 声明）与资源。
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
    // JVM 版 JNA jar 不含 Android 的 libjnidispatch.so，替换为 aar。
    api(project(":app-mcp")) {
        exclude(group = "net.java.dev.jna", module = "jna")
    }
    api("net.java.dev.jna:jna:5.18.1@aar")
    api("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.10.2")
    implementation("androidx.lifecycle:lifecycle-process:2.9.4")
    // 公开 API（enableWhile）用到 LifecycleOwner / Lifecycle
    api("androidx.lifecycle:lifecycle-common:2.9.4")
    // 唤醒：WakeReceiver → 加急 WorkManager 任务（WakeWorker）
    api("androidx.work:work-runtime-ktx:2.10.1")

    testImplementation("junit:junit:4.13.2")
    testImplementation("org.robolectric:robolectric:4.14.1")
    testImplementation("androidx.test:core:1.6.1")
    testImplementation("androidx.work:work-testing:2.10.1")
    testImplementation("org.jetbrains.kotlinx:kotlinx-coroutines-test:1.10.2")
}
