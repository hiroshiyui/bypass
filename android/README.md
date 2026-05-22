# bypass — Android app (Phase 8.2.b)

A Compose UI on top of [`crates/bypass-ffi/`](../crates/bypass-ffi/).
Searches your `bypass` store, copies passwords to the clipboard,
generates new ones — all crypto delegates to
[OpenKeychain](https://github.com/open-keychain/open-keychain) via
its OpenPGP AIDL service. Same trust model as the desktop CLI's
`gpg` subprocess delegation ([ADR-0001](../doc/adr/0001-platform-delegated-crypto.md));
realised for Android per [ADR-0024](../doc/adr/0024-android-ffi-via-uniffi.md) +
[ADR-0025](../doc/adr/0025-android-ui-architecture.md).

## Status

**8.2.b — OpenKeychain integration (happy-path).** App
launches, binds OpenKeychain on first use, every CRUD op
works end-to-end as long as OpenKeychain has a hot passphrase
cache. If the cache has expired, encrypt / decrypt surface
an actionable `BypassException.Crypto("OpenKeychain needs
user interaction … unlock the key, then retry")` in a
Snackbar; the user opens OpenKeychain, unlocks, returns to
bypass, and retries. No async PendingIntent bridge yet —
**8.2.b.ii** is where that lives if real users hit the cold-
cache path often.

## Prerequisites

- **OpenKeychain installed on the device.** [Play Store](https://play.google.com/store/apps/details?id=org.sufficientlysecure.keychain) or
  [F-Droid](https://f-droid.org/packages/org.sufficientlysecure.keychain/).
  At least one OpenPGP key in its keyring, addressable by
  fingerprint or email.
- **OpenKeychain passphrase cache** set to a long TTL under
  Settings → Password cache → "Cache passphrases" (default
  is reasonable; "never" if you trust the device fully).
  Otherwise every encrypt / decrypt round-trip you do while
  the cache is cold needs a tab over to OpenKeychain to
  unlock + a return tap on bypass.

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

Android Studio's "open existing project" sync also works
— Studio runs the same Gradle tasks under the hood.

## How the Rust ↔ Kotlin glue works

Two Gradle tasks in `app/build.gradle.kts` (ADR-0025
"hand-rolled" decision):

- `cargoNdkBuild` — runs
  `cargo ndk -t arm64-v8a -t armeabi-v7a build --release -p bypass-ffi`
  against the workspace root and drops `libbypass.so` into
  `app/src/main/jniLibs/`.
- `generateUniffiBindings` — depends on `cargoNdkBuild`;
  invokes `cargo run --bin uniffi-bindgen` to emit Kotlin
  sources into `app/build/generated/uniffi/`.

Both wire into AGP's `preBuild` so every Gradle build starts
by regenerating the `.so` + the Kotlin bindings. Drift
between Rust and Kotlin is impossible — every Gradle build
matches the canonical Rust surface in
[`crates/bypass-ffi/src/lib.rs`](../crates/bypass-ffi/src/lib.rs).

## How OpenKeychain integration works

[`crypto/OpenKeychainCrypto.kt`](app/src/main/kotlin/io/bypass/android/crypto/OpenKeychainCrypto.kt)
holds an `OpenPgpServiceConnection` bound at app start (in
`BypassApplication`'s `lazy` repository initialiser). The
binding completes asynchronously; the first encrypt /
decrypt call blocks on a `CountDownLatch` until either the
bind succeeds or a 10 s timeout fires.

Each call dispatches `OpenPgpApi.executeApi()`:

| Result code                          | What we do                                                                        |
| ------------------------------------ | --------------------------------------------------------------------------------- |
| `RESULT_CODE_SUCCESS`                | Return the bytes.                                                                 |
| `RESULT_CODE_ERROR`                  | Throw `BypassException.Crypto` with OpenKeychain's error message.                 |
| `RESULT_CODE_USER_INTERACTION_REQUIRED` | Throw `BypassException.Crypto("…unlock the key, then retry")` — see 8.2.b.ii. |

The `<queries>` block in `AndroidManifest.xml` declares
`org.sufficientlysecure.keychain` so Android 11+ package
visibility lets us bind to it. Without that, `bindToService`
silently fails on API 30+.

## What's missing (and why)

- **Async PendingIntent bridge** (8.2.b.ii). On
  `RESULT_CODE_USER_INTERACTION_REQUIRED` the proper UX is
  to launch the returned `PendingIntent` via
  `ActivityResultLauncher`, suspend the FFI call across the
  user-confirm round-trip, and resolve once OpenKeychain
  hands control back. Today we throw — the user has to
  unlock manually and retry. Add when the workflow
  actually annoys real users.
- **Instrumented tests** (Espresso / Compose UI testing).
  Require a green local Studio sync first to anchor
  expectations.
- **Polished icon.** The placeholder is a vector "by" mark
  on an indigo background. Replace before any Play Store
  submission.
- **Sync** between Android and other devices. ROADMAP's
  8.2.c plan defers libgit2-on-NDK; today an Android device
  can't push to a paired desktop. Manual export / import is
  the fallback.

## Troubleshooting

- **"OpenKeychain AIDL service did not bind within 10s"** →
  OpenKeychain isn't installed, or the `<queries>` block in
  the manifest got stripped (check your build outputs).
  Install OpenKeychain and re-launch.
- **"OpenKeychain needs user interaction"** → expected when
  the passphrase cache has expired. Open OpenKeychain,
  unlock the key, return to bypass, retry the action.
  Long-term, raise the cache TTL.
- **Gradle sync fails on `cargoNdkBuild`** → `cargo-ndk`
  isn't on `$PATH`, or `ANDROID_NDK_HOME` is unset. See
  the "Build" section above.
- **`UnsatisfiedLinkError: bypass`** at runtime → the `.so`
  files in `app/src/main/jniLibs/` are missing or stale.
  Run `./gradlew cargoNdkBuild` manually to refresh.
- **Studio complains about Kotlin / Compose version
  mismatch** → bump `gradle/libs.versions.toml`. Kotlin and
  the Compose Compiler are coupled since K2; bump both
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
    ├── build.gradle.kts             # AGP + Compose + cargoNdkBuild / generateUniffiBindings
    ├── proguard-rules.pro
    └── src/main/
        ├── AndroidManifest.xml      # <queries> for OpenKeychain
        ├── kotlin/io/bypass/android/
        │   ├── BypassApplication.kt
        │   ├── MainActivity.kt
        │   ├── crypto/OpenKeychainCrypto.kt
        │   ├── repository/BypassRepository.kt
        │   ├── ui/BypassNavHost.kt
        │   ├── ui/theme/Theme.kt
        │   └── ui/screens/{Init,List,Show,Insert,Generate}Screen.kt
        └── res/
            ├── values/{strings.xml, themes.xml}
            ├── drawable/ic_launcher_{background,foreground}.xml
            └── mipmap-anydpi-v26/ic_launcher.xml
```
