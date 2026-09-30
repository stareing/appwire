// 控制台示例：连接 Host，注册几个工具，Ctrl+C 退出。
plugins {
    kotlin("jvm")
    kotlin("plugin.serialization")
    application
}

kotlin {
    compilerOptions { jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17) }
}

java {
    sourceCompatibility = JavaVersion.VERSION_17
    targetCompatibility = JavaVersion.VERSION_17
}

dependencies {
    implementation(project(":app-mcp"))
}

application {
    mainClass.set("dev.appmcp.sample.MainKt")
}
