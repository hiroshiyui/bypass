// SPDX-License-Identifier: GPL-3.0-or-later
//
// The Android app module — Compose UI + UniFFI bindings to
// `bypass-ffi` over JNA. Two hand-rolled `Exec` tasks
// (`cargoNdkBuild` + `generateUniffiBindings`) wire into
// `preBuild` so every Gradle build regenerates the .so files
// and Kotlin bindings from the canonical Rust source (ADR-0025).

import org.gradle.api.tasks.Exec

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
}

android {
    namespace = "io.bypass.android"
    compileSdk = libs.versions.android.compile.sdk.get().toInt()

    defaultConfig {
        applicationId = "io.bypass.android"
        minSdk = libs.versions.android.min.sdk.get().toInt()
        targetSdk = libs.versions.android.target.sdk.get().toInt()
        versionCode = 1
        versionName = "0.1.0"
        // Only the two ABIs cargo-ndk + the FFI crate currently
        // build for. Adding x86_64 emulator support is a 8.2.b
        // concern (one more cargo-ndk -t plus Rust target add).
        ndk {
            abiFilters += listOf("arm64-v8a", "armeabi-v7a")
        }
        // Mirror the CLI's repro-build flags where it makes sense.
        vectorDrawables { useSupportLibrary = true }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro",
            )
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions {
        jvmTarget = "17"
    }

    buildFeatures {
        compose = true
    }

    // Generated UniFFI Kotlin lands here, registered as a Kotlin
    // source root so Studio sees the synthetic classes without
    // manual import resolution.
    sourceSets {
        getByName("main") {
            kotlin.srcDir(layout.buildDirectory.dir("generated/uniffi"))
            jniLibs.srcDir("src/main/jniLibs")
        }
    }

    packaging {
        resources {
            excludes += "/META-INF/{AL2.0,LGPL2.1}"
        }
    }
}

dependencies {
    implementation(libs.core.ktx)
    implementation(libs.lifecycle.runtime.ktx)
    implementation(libs.lifecycle.runtime.compose)
    implementation(libs.lifecycle.viewmodel.compose)
    implementation(libs.activity.compose)
    implementation(platform(libs.compose.bom))
    implementation(libs.compose.ui)
    implementation(libs.compose.ui.graphics)
    implementation(libs.compose.ui.tooling.preview)
    implementation(libs.compose.material3)
    implementation(libs.navigation.compose)
    // UniFFI's Kotlin output uses JNA to talk to libbypass.so.
    implementation(libs.jna) { artifact { type = "aar" } }
    debugImplementation(libs.compose.ui.tooling)
}

// ----- UniFFI ↔ Gradle integration ---------------------------------

// Repo root = parent of android/. Both tasks run from there so
// `cargo` finds the workspace `Cargo.toml`.
val repoRoot = rootProject.projectDir.parentFile

val cargoNdkBuild = tasks.register<Exec>("cargoNdkBuild") {
    description = "Cross-compile bypass-ffi for the Android ABIs and copy the .so files into src/main/jniLibs/."
    group = "build"
    workingDir = repoRoot
    commandLine(
        "cargo", "ndk",
        "-t", "arm64-v8a",
        "-t", "armeabi-v7a",
        "-o", file("src/main/jniLibs").absolutePath,
        "build", "--release", "-p", "bypass-ffi",
    )
    inputs.dir(file("$repoRoot/crates/bypass-ffi/src"))
    inputs.file(file("$repoRoot/crates/bypass-ffi/Cargo.toml"))
    outputs.dir("src/main/jniLibs")
}

val generateUniffiBindings = tasks.register<Exec>("generateUniffiBindings") {
    description = "Emit Kotlin bindings for bypass-ffi into build/generated/uniffi/."
    group = "build"
    dependsOn(cargoNdkBuild)
    workingDir = repoRoot
    val outDir = layout.buildDirectory.dir("generated/uniffi").get().asFile.absolutePath
    commandLine(
        "cargo", "run", "-q", "-p", "bypass-ffi", "--bin", "uniffi-bindgen",
        "--",
        "generate",
        "--library", "$repoRoot/target/aarch64-linux-android/release/libbypass.so",
        "--language", "kotlin",
        "--out-dir", outDir,
    )
    inputs.dir(file("src/main/jniLibs"))
    outputs.dir(outDir)
}

// `preBuild` is AGP's hook that runs before everything else.
tasks.named("preBuild") {
    dependsOn(generateUniffiBindings)
}
