import java.util.Properties
import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("rust")
}

val tauriProperties = Properties().apply {
    val propFile = file("tauri.properties")
    if (propFile.exists()) {
        propFile.inputStream().use { load(it) }
    }
}

// HTTPS と relay の証明書の検証（rustls-platform-verifier）の Kotlin 部品。Rust の rustls-platform-verifier-android と
// 同じ版でないと実行時に落ちるため、src-tauri の Cargo.lock から版を読む（#1195）。
val rustlsPlatformVerifierVersion = providers.fileContents(layout.projectDirectory.file("../../../Cargo.lock"))
    .asText.get().lines()
    .dropWhile { it.trim() != "name = \"rustls-platform-verifier-android\"" }
    .drop(1).first().substringAfter('"').substringBefore('"')

repositories {
    maven { url = uri("https://github.com/rustls/rustls-platform-verifier/raw/maven-archive/android-release-support/maven/") }
}

android {
    // #1193 D7: target は Google Play の要件の API 36、minSdk は 29（tauri.android.conf.json）。
    compileSdk = 36
    namespace = "app.kukuri.android"
    defaultConfig {
        manifestPlaceholders["usesCleartextTraffic"] = "false"
        applicationId = "app.kukuri.android"
        minSdk = 29
        targetSdk = 36
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
            ndk.debugSymbolLevel = "FULL"
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
    // ランチャーアイコンは `tauri icon` の出力をそのまま使う（権利は docs/ASSET_MANIFEST.json で管理）。
    sourceSets["main"].res.srcDir("../../../icons/android")
}

kotlin {
    compilerOptions {
        jvmTarget = JvmTarget.JVM_1_8
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
    implementation("org.rustls:rustls-platform-verifier:$rustlsPlatformVerifierVersion")
    testImplementation("junit:junit:4.13.2")
    androidTestImplementation("androidx.test.ext:junit:1.1.4")
    androidTestImplementation("androidx.test.espresso:espresso-core:3.5.0")
}

apply(from = file("tauri.build.gradle.kts"))
