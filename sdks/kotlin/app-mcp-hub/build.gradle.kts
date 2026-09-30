// Kotlin/JVM 库：Hub SDK（Agent 端）。uniffi 生成的绑定（dev.appmcp.hub.ffi）+ 惯用封装（dev.appmcp.hub）。
// 绑定与原生库由 bindings/hub-uniffi/scripts/generate.sh 生成到 src/generated/。
plugins {
    kotlin("jvm")
    kotlin("plugin.serialization")
    `java-library`
}

kotlin {
    compilerOptions {
        jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17)
    }
}

java {
    sourceCompatibility = JavaVersion.VERSION_17
    targetCompatibility = JavaVersion.VERSION_17
}

sourceSets {
    main {
        kotlin.srcDir("src/generated/kotlin")
        // 本机平台的原生库（JNA 从 classpath 的 <os>-<arch>/ 目录加载）
        resources.srcDir("src/generated/resources")
    }
}

dependencies {
    api("net.java.dev.jna:jna:5.18.1")
    api("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.10.2")
    api("org.jetbrains.kotlinx:kotlinx-serialization-json:1.9.0")

    testImplementation(kotlin("test"))
    // 集成测试：同进程用 App 端 SDK 连上嵌入式 Hub
    testImplementation(project(":app-mcp"))
}

tasks.test {
    useJUnitPlatform()
    testLogging {
        events("passed", "skipped", "failed")
        showStandardStreams = true
        exceptionFormat = org.gradle.api.tasks.testing.logging.TestExceptionFormat.FULL
    }
}
