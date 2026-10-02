// 独立 Hub App 原型（TASKS 4g d）：内嵌 Hub（:app-mcp-hub-android），导出供 Agent 绑定的 Service（Intent 动作
// `dev.appmcp.HUB`）；Binder 上只交换一个 socketpair fd，fd 上跑 MCP（每行一条 JSON-RPC，复用 Hub 的 MCP 出口）。
// Hub 再按名寻址（spec/naming.md 4.2）以 bindService 连接目标 App。不常驻：没有 Agent 绑定时 Service 销毁、Hub 关闭，
// 进程交给系统回收；不加前台服务。
//
// 原生库须包含 MCP 出口（mcp-server）：Hub App 用自己的一份 .so（src/main/jniLibs/<abi>/，不纳入版本控制），
// 与 :app-mcp-hub-android 里给嵌入式 Hub 的精简库（generate.sh --android，不含 MCP 出口）分开编译：
//   bash bindings/hub-uniffi/scripts/generate.sh --release --hub-app --abi arm64-v8a
//   ./gradlew :hub-app-android:assembleRelease -Pappmcp.abis=arm64-v8a
// 两份同名（uniffi 按库名加载）。AGP 8.13 实测：mergeNativeLibs 遇到同名时选 app 模块的那份，并警告"未来版本可能报错"；
// 这里以 packaging.jniLibs.pickFirsts 显式声明（app 模块的 jniLibs 是第一个输入），消除警告。缺少所请求 ABI 的这一份时
// 构建失败（checkHubAppNativeLibs），不会静默打进精简库；运行时 HubService 仍检查 hub_features().mcp_server，
// 缺少时向 Agent 报 HUB_UNSUPPORTED。
plugins {
    id("com.android.application")
    kotlin("android")
}

val allAbis = listOf("arm64-v8a", "armeabi-v7a", "x86_64", "x86")
val hubLib = "libapp_mcp_hub_uniffi.so"
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
            // 本模块 jniLibs 中带 mcp-server 的 Hub 库优先于 :app-mcp-hub-android 的同名精简库。
            pickFirsts += "lib/*/$hubLib"
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

// 打包前核对：每个请求的 ABI 都有本模块自己的 Hub 库（带 mcp-server），否则失败并给出编译命令。
val checkHubAppNativeLibs by tasks.registering {
    val jniDir = layout.projectDirectory.dir("src/main/jniLibs")
    val requested = abis
    doLast {
        val missing = requested.filterNot { jniDir.file("$it/$hubLib").asFile.isFile }
        if (missing.isNotEmpty()) {
            throw GradleException(
                "Hub App 缺少带 mcp-server 的原生库（${missing.joinToString()}）：" +
                    "bash bindings/hub-uniffi/scripts/generate.sh --release --hub-app " +
                    missing.joinToString(" ") { "--abi $it" },
            )
        }
    }
}
tasks.matching { it.name.startsWith("merge") && it.name.endsWith("NativeLibs") }.configureEach {
    dependsOn(checkHubAppNativeLibs)
}

dependencies {
    implementation(project(":app-mcp-hub-android"))

    testImplementation("junit:junit:4.13.2")
    testImplementation("org.robolectric:robolectric:4.14.1")
    testImplementation("androidx.test:core:1.6.1")
}
