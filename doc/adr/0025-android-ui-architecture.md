<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Android UI architecture: Compose, single Activity, manual DI

* Status: accepted
* Date: 2026-05-22
* Deciders: hiroshiyui

## Context and Problem Statement

[Phase 8.1](0024-android-ffi-via-uniffi.md) landed the Rust
FFI for Android: `bypass-ffi` cdylib, UniFFI-generated Kotlin
surface, CI cross-compile. Phase 8.2 is the Android app on
top — the Compose UI that the user actually touches.

This ADR settles five load-bearing decisions for that app
before any Kotlin lands:

1. **UI architecture** — Compose-only, Activity / Fragment
   mix, or Compose-with-navigation-component?
2. **Dependency injection** — Hilt, Koin, or manual?
3. **`Crypto` for the 8.2.a slice** — real OpenKeychain
   client, or a stub deferred to 8.2.b?
4. **Gradle ↔ UniFFI integration** — a third-party plugin
   (`mozilla/cargo-uniffi` and friends) or hand-rolled tasks?
5. **CI** — Android Gradle build now, or defer to 8.2.b after
   the first local Studio sync proves the scaffold is right?

The user picked 8.2.a (UI scaffold + stub `Crypto` for
8.2.b), package `io.bypass`, `minSdk` 26, and hand-rolled
Gradle tasks before this ADR was written. The ADR records the
ground rules so 8.2.b can land without re-litigating them.

## Considered Options

**UI architecture:**

* **Compose-only, single `MainActivity`, `NavHost` for
  navigation.** Standard Jetpack Compose pattern; Android
  Studio's project wizard generates exactly this skeleton
  for a new "Empty Activity" project. Fragments would add a
  layer with no value for the five screens we ship.
* Activity-per-screen. Forces every transition through the
  intent system, breaks shared `ViewModel` state, and is
  the pattern Android tooling has been steering away from
  since 2018.
* Fragments inside a host Activity. Compose can host
  Fragments, but the inverse direction is the canonical
  one; pure-Compose avoids the bridge.

**Dependency injection:**

* **Manual DI.** A custom `Application` subclass holds the
  `BypassRepository` singleton; ViewModels grab it from
  `(LocalContext.current.applicationContext as
  BypassApplication).repository`. Five screens, one
  long-lived object — Hilt would be overkill.
* Hilt. Standard Google recommendation; sound choice for
  multi-module apps with deep DI graphs. We have one
  repository and one stub crypto; the boilerplate cost
  exceeds the dependency-graph cost.
* Koin. Lighter than Hilt; still a runtime DI framework
  whose overhead we'd be paying for one binding.

**`Crypto` impl for 8.2.a:**

* **Stub `Crypto` that throws** `BypassException.Crypto`
  on encrypt/decrypt. Lets the UI scaffold build, lets the
  Init / List / Find / Rm flows work end-to-end without an
  OpenKeychain dep, and surfaces the expected error on
  Show / Insert / Generate so the FFI error round-trip is
  observable. **Defers OpenKeychain to 8.2.b**.
* Real OpenKeychain client in 8.2.a. Adds the
  `org.openintents.openpgp:openpgp-api` dep + an
  AIDL-binding flow + the async/sync bridge needed to
  satisfy UniFFI's synchronous foreign-trait surface.
  Substantial, hard to verify without a real device or
  emulator + OpenKeychain installed, and impossible to
  validate from this dev machine.
* No `Crypto` at all in 8.2.a — `init` / `list` work,
  Insert / Show / Generate screens stay greyed out.
  Worse UX than the stub-with-error path; the stub at
  least proves the round-trip wiring.

**Gradle ↔ UniFFI integration:**

* **Hand-rolled Gradle tasks** in `app/build.gradle.kts`.
  Two `Exec` tasks (`cargoNdkBuild`,
  `generateUniffiBindings`) wire into `preBuild`. ~40 lines
  of DSL; transparent; the user can read the build
  end-to-end without consulting a third-party plugin's docs.
* `mozilla-actions/cargo-uniffi` / community Gradle plugin.
  Less DSL to write; adds a Maven plugin-portal dep we have
  to track + pin. Plugin maintenance is itself a signal we'd
  be coupling to.
* Pre-built `.so` + pre-generated Kotlin committed to the
  repo. Removes the toolchain dep at build time; introduces
  drift between Rust and Kotlin. The hand-rolled tasks make
  drift impossible — Gradle builds always regenerate.

**CI:**

* **Defer Android Gradle build to 8.2.b.** Adds 5–10 min
  per CI run; blocks merges if Gradle sync fails for any
  reason we can't reproduce locally without Android Studio.
  The existing `android-ffi` cross-compile job already
  validates the FFI side, which is the part with real CI
  failure modes. The Gradle build is the thing the user is
  going to verify first by opening the project in Studio
  anyway.
* Add the Gradle build to CI now. Catches regressions
  earlier, but until 8.2.b confirms the scaffold is
  buildable on at least one machine, CI failures would be
  noise we can't act on.

## Decision Outcome

- **UI architecture**: Compose-only, single
  `MainActivity`, `NavHost` for navigation. Five routes:
  `init`, `list`, `show/{path}`, `insert`, `generate`.
- **Dependency injection**: **manual**. A
  `BypassApplication: android.app.Application` subclass
  holds the `BypassRepository` as a `lazy` property;
  ViewModels read it through the application context.
  Hilt re-evaluated only if the graph grows past ~5
  bindings.
- **`Crypto` in 8.2.a: stub.** Throws
  `BypassException.Crypto("OpenKeychain integration lands
  in 8.2.b — encrypt()/decrypt() is stubbed in this
  build.")`. Lives at
  `android/app/src/main/kotlin/io/bypass/android/crypto/StubCrypto.kt`.
  The real OpenKeychain client is 8.2.b's deliverable.
- **Gradle ↔ UniFFI integration**: hand-rolled `Exec` tasks
  (`cargoNdkBuild` + `generateUniffiBindings`) in
  `app/build.gradle.kts`. Generated Kotlin lands at
  `app/build/generated/uniffi/`, registered as a Kotlin
  source root.
- **CI**: no Android Gradle build in 8.2.a. The existing
  `android-ffi` job in `.github/workflows/ci.yml` validates
  the FFI cross-compile; 8.2.b adds the Gradle build after
  the first Studio sync confirms the project layout.
- **Toolchain pins** live in
  `android/gradle/libs.versions.toml`. AGP 8.7.x, Kotlin
  2.0.21 (Compose Compiler since K2), Compose BOM
  2024.12.01, Java 17 toolchain, `minSdk` 26, `targetSdk`
  35, navigation-compose 2.8.x, lifecycle-viewmodel-compose
  2.8.x, activity-compose 1.9.x. Catalogue means a bump is
  one file.
- **App-scoped storage**: `BypassStore.open(ctx.filesDir
  .resolve("store").absolutePath, crypto)` at app start.
  Android's per-app sandbox handles the rest (no symlink
  threat, no shared-host concern; same rationale as
  ADR-0024's `AppStorage` implementation).
- **Package**: `io.bypass`. App ID and Kotlin package
  identical; the application class is
  `io.bypass.android.BypassApplication`.

## Consequences

### Good

- Compose / single-Activity / NavHost / manual-DI / per-
  screen ViewModel / Repository is *the* canonical
  Jetpack-stable pattern as of 2024+. First contact with
  Android Studio finds nothing surprising.
- The stub-`Crypto` split lets 8.2.a ship something the
  user can build and open in Studio without any
  OpenKeychain dep on the device. Real wiring lands in
  8.2.b with its own ADR.
- Hand-rolled Gradle tasks are 40 lines of DSL the user
  can read top-to-bottom; no third-party plugin to track.
  Drift between Rust and Kotlin is impossible by
  construction — every Gradle build regenerates the
  bindings.
- App-scoped storage matches `AppStorage`'s ADR-0024
  threat-model assumption exactly; no surprise between
  the FFI's expectations and the Android side.
- Toolchain pins in the version catalog make a future bump
  a single-file edit.

### Bad

- 8.2.a has no working crypto. Users who pull `main` and
  open the app see Init / List / Find / Rm work and
  Show / Insert / Generate fail with a clear stub error.
  Required tooling story until 8.2.b lands; mitigated by
  the explicit error message + the README's
  "Limitations of 8.2.a" section.
- Manual DI means `BypassApplication.repository` is a
  global state hook. Acceptable for one repository;
  becomes painful if the graph grows. Trigger to swap in
  Hilt is "second long-lived singleton".
- Hand-rolled Gradle tasks require `cargo` +
  `cargo-ndk` + the Android NDK on the user's `PATH` when
  they run a Gradle build. Documented in
  `android/README.md`; same toolchain CI's `android-ffi`
  job already needs.
- CI catches no regressions on the Android side until
  8.2.b. Mitigation: the Gradle scaffold is small, every
  file is canonical pattern, and the user's first Studio
  sync is itself a more thorough check than CI could be.

## Confirmation

- The Gradle project at `android/` exists; `gradle sync`
  in Android Studio completes cleanly on the user's
  machine (verified once after merge — this ADR is in
  effect after that first sync).
- `android/app/src/main/kotlin/io/bypass/android/crypto/StubCrypto.kt`
  is the only `Crypto` implementation in the repo.
  Replacing it requires a superseding ADR.
- The two Gradle tasks (`cargoNdkBuild`,
  `generateUniffiBindings`) live in
  `android/app/build.gradle.kts`. Removing or replacing
  them with a third-party plugin requires a superseding
  ADR.
- A future change to the package name, `minSdk`, or the
  manual-DI choice requires a superseding ADR.

## Related ADRs

- [ADR-0001](0001-platform-delegated-crypto.md): the
  platform-delegated-crypto posture this ADR builds on.
  Stub `Crypto` in 8.2.a temporarily breaks the delegation
  promise (no real OpenPGP backend behind it); 8.2.b will
  honour the promise via OpenKeychain.
- [ADR-0003](0003-workspace-split-core-cli.md): why the
  Android app *can't* depend on `bypass-cli`. We consume
  `bypass-ffi`, the third workspace crate, exactly the way
  ADR-0003 anticipated.
- [ADR-0024](0024-android-ffi-via-uniffi.md): defines the
  FFI surface this UI consumes. `BypassStore`, `Crypto`,
  and `BypassException` are the contract.
- [ADR-0021](0021-release-packaging.md): the Android side
  is *not yet* in scope for release packaging. Play Store
  / Google Play signing is a future Phase 9 concern.
