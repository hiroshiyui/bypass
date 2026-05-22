# bypass — Android app (Phase 8.2.a)

A Compose UI on top of [`crates/bypass-ffi/`](../crates/bypass-ffi/).
Searches your `bypass` store, copies passwords to the clipboard,
generates new ones — all crypto stays in OpenKeychain via the
FFI's `Crypto` callback (once 8.2.b lands; 8.2.a uses a stub).

Locks the architecture per [ADR-0025](../doc/adr/0025-android-ui-architecture.md);
FFI wire shape per [ADR-0024](../doc/adr/0024-android-ffi-via-uniffi.md).

## Status

**8.2.a — UI scaffold + stub `Crypto`.** App launches, Init /
List / Find / Rm flows work end-to-end. Insert / Show /
Generate surface a `BypassException.Crypto("OpenKeychain
integration lands in 8.2.b — …")` in a Snackbar; proves the
FFI error round-trip is wired but doesn't store / retrieve
real data yet.

**8.2.b (next)** replaces `StubCrypto` with an OpenKeychain
AIDL client built on `org.openintents.openpgp:openpgp-api`.

## Build

You need:
- Android Studio (Iguana / Jellyfish or later; AGP 8.7.x).
- The Rust toolchain on `$PATH` plus `cargo-ndk` and the
  Android targets:
  ```sh
  rustup target add aarch64-linux-android armv7-linux-androideabi
  cargo install --locked cargo-ndk
  ```
- The Android NDK installed via Studio's SDK Manager (any
  recent r26+ NDK works).

Then:
```sh
cd android/
./gradlew assembleDebug              # builds the debug APK
./gradlew installDebug               # installs to a connected device / emulator
adb shell am start -n io.bypass.android/io.bypass.android.MainActivity
```

A normal Android Studio "open existing project" sync also
works — Studio runs the same Gradle tasks under the hood. On
first sync, Studio may prompt to install the AGP version
declared in `gradle/libs.versions.toml`; accept.

## How the Rust ↔ Kotlin glue works

Two Gradle tasks in `app/build.gradle.kts` (ADR-0025
"hand-rolled" decision):

- `cargoNdkBuild` — runs
  `cargo ndk -t arm64-v8a -t armeabi-v7a build --release -p bypass-ffi`
  against the workspace root and drops `libbypass.so` into
  `app/src/main/jniLibs/`.
- `generateUniffiBindings` — depends on `cargoNdkBuild`;
  invokes `cargo run --bin uniffi-bindgen` to emit Kotlin
  sources into `app/build/generated/uniffi/`, then registers
  that dir as a Kotlin source root.

Both wire into AGP's `preBuild` so every Gradle build starts
by regenerating the `.so` + the Kotlin bindings. Drift
between Rust and Kotlin is impossible — the bindings always
match the Rust surface in `crates/bypass-ffi/src/lib.rs`.

## What's missing (and why)

- **OpenKeychain client** (8.2.b). `StubCrypto` is the only
  `Crypto` implementation today; it throws on every call.
  Real crypto needs the async-AIDL-PendingIntent flow from
  OpenKeychain bridged onto UniFFI's synchronous foreign-
  trait surface; that's its own design problem.
- **Instrumented tests** (Espresso / Compose UI testing).
  Until the Studio sync is green on at least one user
  machine, instrumented tests are premature.
- **Android Gradle build in CI.** Adds 5–10 min per CI run
  and would block on the first Studio sync producing a green
  project. Add in 8.2.b after the user confirms the scaffold
  is correct.
- **Polished icon.** The placeholder is a vector "by" mark on
  an indigo background. Replace before any Play Store
  submission.
- **Sync** between Android and other devices. ROADMAP's
  8.2 plan defers libgit2-on-NDK; today an Android device
  can't push to a paired desktop. Manual export / import is
  the fallback until 8.2.c (if ever) lands a libgit2 cross-
  compile.

## Troubleshooting

- **Gradle sync fails on `cargoNdkBuild`** → `cargo-ndk`
  isn't on `$PATH`, or `ANDROID_NDK_HOME` is unset. See the
  "You need" list above.
- **`UnsatisfiedLinkError: bypass`** at runtime → the `.so`
  files in `app/src/main/jniLibs/` are missing or stale.
  Run `./gradlew cargoNdkBuild` manually to refresh.
- **Init succeeds but Insert / Show fail with a Crypto
  error** → expected for 8.2.a. The stub `Crypto` throws on
  every call; full functionality lands in 8.2.b.
- **Studio complains about Kotlin / Compose version
  mismatch** → bump `gradle/libs.versions.toml`. Kotlin and
  the Compose Compiler are now coupled (since K2); bump both
  together.

## Layout

```
android/
├── settings.gradle.kts
├── build.gradle.kts                # root: plugin declarations
├── gradle.properties
├── gradle/libs.versions.toml       # version catalog (single source of truth)
├── gradle/wrapper/                  # gradle 8.10 wrapper
├── gradlew, gradlew.bat
└── app/
    ├── build.gradle.kts             # AGP + Compose + the cargoNdkBuild / generateUniffiBindings tasks
    ├── proguard-rules.pro
    └── src/main/
        ├── AndroidManifest.xml
        ├── kotlin/io/bypass/android/
        │   ├── BypassApplication.kt
        │   ├── MainActivity.kt
        │   ├── crypto/StubCrypto.kt
        │   ├── repository/BypassRepository.kt
        │   ├── ui/BypassNavHost.kt
        │   ├── ui/theme/Theme.kt
        │   └── ui/screens/{Init,List,Show,Insert,Generate}Screen.kt
        └── res/
            ├── values/{strings.xml, themes.xml}
            ├── drawable/ic_launcher_{background,foreground}.xml
            └── mipmap-anydpi-v26/ic_launcher.xml
```
