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
    // WorkManager: the periodic background sync (A315).
    implementation("androidx.work:work-runtime-ktx:2.10.5")

    // Firebase Messaging, which `StvdyMessagingService` compiles against.
    //
    // **This declaration is required, and its absence was a real defect.** The comment
    // here used to say the pinned notifications plugin "already brings the Firebase BOM
    // and `firebase-messaging-ktx`", and that is true of the *runtime* classpath and
    // false of the compile one: Gradle's `implementation` dependencies are not exposed
    // to a consumer's compile classpath, only `api` ones are. So
    // `:tauri-plugin-stvdy-mobile` could not resolve `FirebaseMessagingService` or
    // `RemoteMessage`, and `compileReleaseKotlin` failed with
    // `Unresolved reference 'firebase'`. Nothing caught it earlier because the Android
    // build never reached Kotlin compilation — it stopped at the manifest merge, on the
    // namespace collision with the official notification plugin.
    //
    // The versions mirror `tauri-plugin-notifications` 0.5.0-rc.14's own
    // `android/build.gradle.kts`, because a second, lower version is a second version:
    // Gradle resolves a conflict to the highest, so a drift here is silent rather than
    // fatal. If that plugin is bumped, these two lines move with it.
    implementation(platform("com.google.firebase:firebase-bom:34.19.0"))
    implementation("com.google.firebase:firebase-messaging-ktx:24.1.2")

    implementation(project(":tauri-android"))
}
