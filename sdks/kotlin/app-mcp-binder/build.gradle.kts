// Android 库：Binder 上"一次绑定只交换一个 fd"的共用实现（spec/naming.md 4.2），供 App 端（ToolsService）、
// Hub 端（按名拨号）、独立 Hub App 与 Agent 客户端共用。不含原生库，不依赖其他 SDK 模块。
plugins {
    id("com.android.library")
    kotlin("android")
}

android {
    namespace = "dev.appmcp.binder"
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
    testImplementation("junit:junit:4.13.2")
    testImplementation("org.robolectric:robolectric:4.14.1")
    testImplementation("androidx.test:core:1.6.1")
}
