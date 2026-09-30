// verify.sh 使用：用真实的 androidx.appfunctions 与 KSP 编译 kotlin-appfunctions target 的输出。
plugins {
    id("com.android.library")
    kotlin("android")
    kotlin("plugin.serialization")
    id("com.google.devtools.ksp")
}

android {
    namespace = "appmcp.generated.shop"
    compileSdk = 36
    defaultConfig { minSdk = 26 }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

kotlin {
    compilerOptions { jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17) }
}

ksp { arg("appfunctions:aggregateAppFunctions", "true") }

dependencies {
    implementation("androidx.appfunctions:appfunctions:1.0.0-alpha12")
    ksp("androidx.appfunctions:appfunctions-compiler:1.0.0-alpha12")
    implementation("org.jetbrains.kotlinx:kotlinx-serialization-json:1.9.0")
    // KSP 生成的服务类直接使用 kotlinx.coroutines
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.10.2")
}
