// verify.sh 使用：编译并运行 kotlin target 的输出（JVM）。
plugins {
    kotlin("jvm")
    kotlin("plugin.serialization")
    application
}

dependencies {
    implementation("org.jetbrains.kotlinx:kotlinx-serialization-json:1.9.0")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.10.2")
}

kotlin {
    compilerOptions { allWarningsAsErrors.set(true) }
}

application { mainClass.set("verify.MainKt") }
