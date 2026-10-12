// The Android library the plugin's Kotlin compiles into. `tauri_plugin::Builder`'s
// `android_path("android")` copies the Tauri API into `android/.tauri/tauri-api` and
// emits `cargo:android_library_path`; the app's Gradle build includes this directory
// as a module, which is what makes `:tauri-android` resolvable below.
plugins {
    id("com.android.library")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "app.stvdy.mobile"
    // Matches the app template's `targetSdk`; a library compiling against an older
    // SDK cannot reference the newer APIs the manifest declares.
    compileSdk = 36

    defaultConfig {
        minSdk = 24
        consumerProguardFiles("consumer-rules.pro")
    }

    buildTypes {
        release {
            isMinifyEnabled = false
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_1_8
        targetCompatibility = JavaVersion.VERSION_1_8
    }
    kotlin {
        compilerOptions {
            jvmTarget = org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_1_8
        }
    }
}

dependencies {
    implementation("androidx.core:core-ktx:1.17.0")
    implementation("androidx.appcompat:appcompat:1.8.0")
    implementation("com.fasterxml.jackson.core:jackson-databind:2.22.2")
    // WorkManager: the periodic background sync (A315). No Firebase dependency is
    // declared here — the pinned notifications plugin already brings the Firebase
    // BOM and `firebase-messaging-ktx` when its `push-notifications` feature is on
    // (`src-tauri/Cargo.toml`), and a second declaration would be a second version.
    implementation("androidx.work:work-runtime-ktx:2.10.5")
    implementation(project(":tauri-android"))
}
