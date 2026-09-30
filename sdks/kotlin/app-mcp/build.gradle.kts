// Kotlin/JVM 库：uniffi 生成的绑定（dev.appmcp.ffi）+ 惯用封装（dev.appmcp）。
plugins {
    kotlin("jvm")
    kotlin("plugin.serialization")
    `java-library`
}

kotlin {
    // 不指定 jvmToolchain：用运行 Gradle 的 JDK（本机只有 JRE 21 也能编译 Kotlin）。
    compilerOptions {
        // 生成代码面向 Java 17 字节码，便于 Android 复用
        jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17)
    }
}

java {
    sourceCompatibility = JavaVersion.VERSION_17
    targetCompatibility = JavaVersion.VERSION_17
}

sourceSets {
    main {
        // 由 bindings/uniffi/scripts/generate.sh 生成
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
    testImplementation("org.jetbrains.kotlinx:kotlinx-coroutines-test:1.10.2")
}

tasks.test {
    useJUnitPlatform()
    // 集成测试需要仓库根目录与 cargo target 目录
    systemProperty("appmcp.repoRoot", rootProject.projectDir.resolve("../..").canonicalPath)
    environment("CARGO_TARGET_DIR", System.getenv("CARGO_TARGET_DIR") ?: rootProject.projectDir.resolve("../../target").canonicalPath)
    testLogging {
        events("passed", "skipped", "failed")
        showStandardStreams = true
        exceptionFormat = org.gradle.api.tasks.testing.logging.TestExceptionFormat.FULL
    }
}
