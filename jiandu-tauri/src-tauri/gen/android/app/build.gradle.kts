import java.util.Properties
import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("rust")
}

// 绕过 Tauri CLI 移动端 frontendDist 拷贝缺陷：在 Gradle 合并资源前，
// 把前端源目录（jiandu-tauri/src）灌进独立的 frontend_assets/，
// 再经 sourceSets 声明为额外 assets 源目录（避免与默认 src/main/assets 的 lint 冲突）。
val copyFrontendAssets = tasks.register<Copy>("copyFrontendAssets") {
    // app 模块位于 src-tauri/gen/android/app，前端源在 jiandu-tauri/src，需向上 4 级
    from("../../../../src")
    into("frontend_assets")
}

val tauriProperties = Properties().apply {
    val propFile = file("tauri.properties")
    if (propFile.exists()) {
        propFile.inputStream().use { load(it) }
    }
}

android {
    compileSdk = 37
    namespace = "com.huangjianrong.jiandu"
    defaultConfig {
        manifestPlaceholders["usesCleartextTraffic"] = "false"
        applicationId = "com.huangjianrong.jiandu"
        minSdk = 24
        targetSdk = 37
        versionCode = tauriProperties.getProperty("tauri.android.versionCode", "1").toInt()
        versionName = tauriProperties.getProperty("tauri.android.versionName", "1.0")
    }
    buildTypes {
        getByName("debug") {
            manifestPlaceholders["usesCleartextTraffic"] = "true"
            isDebuggable = true
            isJniDebuggable = true
            isMinifyEnabled = false
            packaging {
                jniLibs.keepDebugSymbols.add("*/arm64-v8a/*.so")
                jniLibs.keepDebugSymbols.add("*/armeabi-v7a/*.so")
                jniLibs.keepDebugSymbols.add("*/x86/*.so")
                jniLibs.keepDebugSymbols.add("*/x86_64/*.so")
            }
        }
        getByName("release") {
            optimization {
               enable = true
            }
            proguardFiles(
                *fileTree(".") {
                  include("**/*.pro")
                  exclude("build/**")
                }.files.toTypedArray()
            )
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_1_8
        targetCompatibility = JavaVersion.VERSION_1_8
    }
    buildFeatures {
        buildConfig = true
    }

    sourceSets {
        getByName("main") {
            assets.srcDir("frontend_assets")
        }
    }

    applicationVariants.all {
        tasks.named("merge${name.replaceFirstChar { it.uppercase() }}Assets") {
            dependsOn(copyFrontendAssets)
        }
    }
}

kotlin {
    compilerOptions {
        jvmTarget = JvmTarget.JVM_1_8
    }
}

// 让所有 lint 任务显式依赖前端拷贝任务，消除 Gradle 隐式依赖校验报错
// （frontend_assets 既是 sourceSet 源目录又被 copyFrontendAssets 产出）
afterEvaluate {
    tasks.configureEach {
        if (name.contains("Lint", ignoreCase = true)) {
            dependsOn(copyFrontendAssets)
        }
    }
}

rust {
    rootDirRel = "../../../"
}

dependencies {
    implementation("androidx.webkit:webkit:1.14.0")
    implementation("androidx.appcompat:appcompat:1.7.1")
    implementation("androidx.activity:activity-ktx:1.10.1")
    implementation("com.google.android.material:material:1.12.0")
    implementation("androidx.lifecycle:lifecycle-process:2.10.0")
    testImplementation("junit:junit:4.13.2")
    androidTestImplementation("androidx.test.ext:junit:1.1.4")
    androidTestImplementation("androidx.test.espresso:espresso-core:3.5.0")
}

apply(from = file("tauri.build.gradle.kts"))
