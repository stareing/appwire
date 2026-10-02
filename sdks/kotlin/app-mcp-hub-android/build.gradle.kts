// Android 库：Hub SDK（Agent 端）的 AAR。复用 :app-mcp-hub 的代码，打包各 ABI 的 libapp_mcp_hub_uniffi.so
// （src/main/jniLibs/<abi>/），由
//   bash bindings/hub-uniffi/scripts/generate.sh --android
// 交叉编译生成（精简组合 mobile,schema-validation，不含 MCP 出口；独立 Hub App 用自己的一份，见 :hub-app-android）。R8 规则随 :app-mcp-hub 的 jar 发布（META-INF/proguard/app-mcp-hub.pro）。
plugins {
    id("com.android.library")
    kotlin("android")
}

android {
    namespace = "dev.appmcp.hub.android"
    compileSdk = 35

    defaultConfig {
        minSdk = 24
    }

    testOptions {
        unitTests.isIncludeAndroidResources = true
        // Robolectric 的 ParcelFileDescriptor.detachFd / adoptFd 需要访问 java.io.FileDescriptor 内部字段（JDK 17+）。
        unitTests.all { it.jvmArgs("--add-opens=java.base/java.io=ALL-UNNAMED") }
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
    api(project(":app-mcp-hub")) {
        exclude(group = "net.java.dev.jna", module = "jna")
    }
    api("net.java.dev.jna:jna:5.18.1@aar")
    api("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.10.2")
    // 按名寻址：bindService + Binder 上交换 fd（AndroidNameService，spec/naming.md 4.2）
    api(project(":app-mcp-binder"))

    testImplementation("junit:junit:4.13.2")
    testImplementation("org.robolectric:robolectric:4.14.1")
    testImplementation("androidx.test:core:1.6.1")
}
