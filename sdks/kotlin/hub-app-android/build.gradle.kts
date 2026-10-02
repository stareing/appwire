// 独立 Hub App 原型（TASKS 4g d）：内嵌 Hub（:app-mcp-hub-android），导出供 Agent 绑定的 Service（Intent 动作
// `dev.appmcp.HUB`）；Binder 上只交换一个 socketpair fd，fd 上跑 MCP（每行一条 JSON-RPC，复用 Hub 的 MCP 出口）。
// Hub 再按名寻址（spec/naming.md 4.2）以 bindService 连接目标 App。不常驻：没有 Agent 绑定时 Service 销毁、Hub 关闭，
// 进程交给系统回收；不加前台服务。
//
// 原生库须包含 MCP 出口：bash bindings/hub-uniffi/scripts/generate.sh --release --android --abi arm64-v8a \
//   --android-features mobile,schema-validation,mcp-server
//   ./gradlew :hub-app-android:assembleRelease -Pappmcp.abis=arm64-v8a
plugins {
    id("com.android.application")
    kotlin("android")
}

val allAbis = listOf("arm64-v8a", "armeabi-v7a", "x86_64", "x86")
val abis = (findProperty("appmcp.abis") as String?)
    ?.split(",")?.map { it.trim() }?.filter { it.isNotEmpty() }
    ?: allAbis

android {
    namespace = "dev.appmcp.hubapp"
    compileSdk = 35

    defaultConfig {
        applicationId = "dev.appmcp.hub.app"
        minSdk = 24
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0"
        ndk { abiFilters += abis }
    }

    packaging {
        jniLibs {
            excludes += listOf("lib/armeabi/**", "lib/mips/**", "lib/mips64/**")
        }
    }

    buildTypes {
        release {
            // 原型：与示例 App 同用 debug keystore 签名（同证书即被示例 App 信任，spec/naming.md 10.2）。
            signingConfig = signingConfigs.getByName("debug")
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"))
        }
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
    implementation(project(":app-mcp-hub-android"))

    testImplementation("junit:junit:4.13.2")
    testImplementation("org.robolectric:robolectric:4.14.1")
    testImplementation("androidx.test:core:1.6.1")
}
