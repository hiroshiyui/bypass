<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Android FFI surface via UniFFI

* Status: accepted
* Date: 2026-05-22
* Deciders: hiroshiyui

## Context and Problem Statement

[Phase 8](../ROADMAP.md#phase-8--android-app) plans a native
Android app on top of `bypass-core`. The
[cross-cutting constraint](../ROADMAP.md#cross-cutting) is that
**OpenPGP stays out of `bypass-core`**: the Android side
delegates crypto to
[OpenKeychain](https://github.com/open-keychain/open-keychain)
the same way the desktop CLI delegates to the `gpg` binary
([ADR-0001](0001-platform-delegated-crypto.md)).

That means the FFI between `bypass-core` (Rust) and the
Android UI (Kotlin) needs:

1. A **bidirectional** boundary — the Rust core needs to *call
   into* Kotlin for `encrypt` / `decrypt`, while Kotlin calls
   into Rust for every store operation.
2. A surface narrow enough that re-wrapping for Swift (iOS,
   future) is cheap if we ever want it.
3. A build pipeline that doesn't pin us to a specific Android
   Studio version or Kotlin DSL.

This ADR commits to the FFI tooling, the surface shape, and
the cross-compile story so the Android UI work in 8.2 can
build against a stable contract.

## Considered Options

**FFI generator:**

* **UniFFI** (Mozilla; powers Firefox iOS / Android, Glean,
  several other Rust-in-mobile projects). Proc-macro mode:
  annotate Rust items with `#[uniffi::Object]` /
  `#[uniffi::export]` / `#[uniffi::Record]`, run a bindgen
  binary that emits Kotlin. Supports callback interfaces
  (foreign-implemented traits) which is exactly the shape we
  need for `Crypto`.
* **JNI by hand** (`jni` crate). Maximum control; ~10× the
  boilerplate; every signature change is a manual edit on
  both sides. Mozilla's experience report on dropping
  hand-JNI for UniFFI is one of the bigger pieces of evidence
  *for* UniFFI.
* **`flapigen-rs`**. Smaller community than UniFFI; similar
  surface; UniFFI's iOS-Swift story is the deciding factor.
* **C-ABI via `cbindgen` + manual JNI shims in Kotlin**.
  Lowest-level; reinvents UniFFI's callback marshalling. No
  reason to.

**UniFFI mode:**

* **Proc-macro mode** — Rust is the single source of truth;
  no `.udl` file to keep in sync. Stable since UniFFI 0.25,
  default since 0.27.
* `.udl` mode — separate IDL file. Older, supported but
  deprecated by the upstream docs. Worse ergonomics, no
  reason to pick.

**Crypto direction:**

* **Crypto is a callback interface** — Rust calls into Kotlin
  for `encrypt` / `decrypt`. Kotlin holds the OpenKeychain
  AIDL client. Plaintext crosses the FFI on the way into
  Rust (`encrypt`); ciphertext crosses on the way back out.
  Matches the CLI's "Crypto is platform-delegated" posture
  exactly.
* Rust-side OpenPGP via `rpgp` or similar. Violates
  [ADR-0001](0001-platform-delegated-crypto.md); pulls a
  large crypto library into our APK; loses OpenKeychain
  integration (the user's existing keyring stays in
  OpenKeychain).

**VCS on Android (8.1 only):**

* **`NoVcs`** for 8.1. The `Store<C, S, V>` orchestrator's
  `V` parameter accepts the no-op VCS impl from Phase 0.5.
  Sync between Android and desktop in 8.1 is manual
  export / import; LAN P2P sync from the Android side waits
  for libgit2-on-NDK to be worth the build complexity.
* `git2` with libgit2 cross-compiled via the NDK. CMake +
  Android NDK clang produces a working `libgit2.so` but each
  bump is a maintenance hit. Deferred; the ROADMAP allows for
  this (8.2's "Optional `git2` integration for sync (libgit2
  with NDK), or defer to manual import/export").

**Storage on Android:**

* **A slim `AppStorage` impl** in `bypass-ffi`, on `std::fs`.
  ~100 lines, no shred-on-remove (Android's per-app
  sandboxing wipes the app's private directory on uninstall
  and isolates it from other apps), no symlink rejection
  (the app's private dir can't have symlinks planted by
  another process). Atomic write (tempfile + rename) and the
  same mode-0600 on tempfile create that the CLI uses.
* Reuse `bypass_cli::storage_fs::StorageFs` — pulls all of
  bypass-cli's deps (libp2p, tokio, notify, arboard, …) into
  the Android APK. Wrong; this is exactly why bypass-ffi is
  a separate crate.
* Extract `StorageFs` into a third "bypass-fs" crate that
  both bypass-cli and bypass-ffi import. Cleanest long-term;
  476-line move with multi-file callers; defer until a third
  user appears (browser extension uses native messaging back
  to the desktop binary, not the trait).

**Generated bindings: ship in-tree or build-time?**

* **Generate at build time** in the Android Gradle project
  (8.2's responsibility): a Gradle task runs
  `cargo run --bin uniffi-bindgen` over the built `.so` and
  emits Kotlin into `app/build/generated/`. Bindings always
  match the Rust surface; nothing committed.
* Commit `bypass.kt` in the repo. Risks drift between Rust
  and Kotlin; ratchets on every UniFFI bump.

**Error mapping:**

* **One `BypassError` enum** with variants matching the
  user-facing failure modes (`NotFound`, `AlreadyExists`,
  `InvalidPath`, `NotInitialized`, `GpgIdMalformed`,
  `Crypto`, `Storage`, `Internal`). Maps via `uniffi::Error`
  to a sealed Kotlin class.
* Pass `StoreError<C, S, V>` through unchanged. UniFFI can't
  represent generics; we'd need to monomorphise. The flat
  enum is honest about which failures the UI needs to
  handle.

## Decision Outcome

- **FFI generator: UniFFI**, proc-macro mode, version **0.28+**
  (pinned to the latest stable line; bumps re-evaluated at
  each minor release).
- **Crypto is a UniFFI callback interface.** Kotlin
  implements; Rust calls. The exact AIDL plumbing to
  OpenKeychain is the Kotlin side's concern (8.2).
- **VCS on Android in 8.1: `NoVcs`.** Defer `git2`; revisit
  in 8.2 if real users need device-side sync without a
  desktop bridge.
- **Storage on Android: in-crate `AppStorage`** under
  `crates/bypass-ffi/src/storage.rs`. ~100 lines of `std::fs`
  with atomic-write + mode-0600. No shred, no symlink check;
  rationale in the file's docstring.
- **Bindings: build-time generation by the Android Gradle
  project.** `bypass-ffi` ships the `uniffi-bindgen` binary;
  the Gradle DSL invokes it. Nothing Kotlin lives in-tree.
- **Error surface: a single `BypassError` enum** with the
  variants above, mapped to a Kotlin sealed class via
  `#[derive(uniffi::Error)]`.
- **Cross-compile targets:**
  `aarch64-linux-android` (modern 64-bit phones) and
  `armv7-linux-androideabi` (older 32-bit). Both built in CI
  on every push via `cargo-ndk`. No on-device run yet; that
  arrives with 8.2's instrumented tests.

### Kotlin-facing surface (illustrative; canonical truth is the
Rust source)

```kotlin
val store = BypassStore.open(
    rootDir = ctx.filesDir.resolve("store").absolutePath,
    crypto  = OpenKeychainCrypto(ctx),  // 8.2 supplies
)
store.init(listOf("you@example.com"))
val plaintext: ByteArray = store.show("email/work")
store.insert("email/work", "hunter2".toByteArray(), overwrite = false)
```

`Crypto` callback shape:

```kotlin
interface Crypto {
    fun encrypt(plaintext: ByteArray, recipients: List<String>): ByteArray
    fun decrypt(ciphertext: ByteArray): ByteArray
}
```

Both raise `BypassError.Crypto` (Kotlin: `BypassException.Crypto`)
on failure; UniFFI handles the exception marshalling.

## Consequences

### Good

- The CLI's `Store<C, S, V>` orchestrator is reused
  unchanged. Adding a new platform (Android, iOS, …) means a
  new concrete `Store<…>` impl + a thin FFI wrapper.
- UniFFI's proc-macro mode keeps Rust as the single source
  of truth for the FFI surface. Adding a new method is one
  Rust function + one annotation; the next Gradle build picks
  it up.
- `bypass-ffi`'s dep graph is small (`bypass-core` + `uniffi`
  + `thiserror`), so the Android APK stays slim. ~500 KB
  stripped vs. ~10 MB if we routed through bypass-cli.
- `Crypto`-as-callback lets the Android side keep using
  OpenKeychain's keyring — same trust model the user already
  signed up for when they installed OpenKeychain.
- The bindings-at-build-time pattern means Rust and Kotlin
  never drift; a CI failure on the bindgen step catches it
  before merge.

### Bad

- `NoVcs` on Android means no device-side commit history in
  8.1. The user can still sync with a desktop via Phase 5.2
  (the desktop side does the pack exchange), but
  Android-to-Android LAN sync is impossible until libgit2
  cross-compiles. Tracked; 8.2 decides whether to invest.
- UniFFI is its own moving target — minor version bumps
  occasionally rename macros. The pin (0.28+) plus the
  bindings-at-build-time pattern catch breakage at build,
  not at runtime.
- The JVM side can't zeroize plaintext bytes (`ByteArray`
  contents are mutable but a future GC pass may copy them
  before we clear them). Same limitation the browser
  extension documents in ADR-0023; the Android side
  inherits it and the 8.2 UI's threat-model note will
  spell that out.
- Two FFI surfaces in the tree (native messaging for the
  browser, UniFFI for Android) means two error vocabularies
  to keep aligned. They serve different consumers and the
  duplication is acceptable.

## Confirmation

- `crates/bypass-ffi/` exists with the structure described
  above; its tests run on the host as part of
  `cargo test --workspace`.
- CI's new `android-ffi` job cross-compiles to both Android
  targets on every push.
- `cargo run -p bypass-ffi --bin uniffi-bindgen -- generate`
  produces Kotlin sources locally — verified by hand; not
  asserted in CI (no Kotlin toolchain in CI for 8.1).
- A future change to the `BypassStore` surface, the
  `BypassError` variants, or the bindings-distribution
  strategy needs a superseding ADR.

## Related ADRs

- [ADR-0001](0001-platform-delegated-crypto.md): why crypto
  stays outside `bypass-core`. The Crypto-as-callback shape
  is the Android realisation of that constraint.
- [ADR-0003](0003-workspace-split-core-cli.md): the
  workspace-portability rule. `bypass-ffi` is the third
  workspace crate (after bypass-core, bypass-cli) and
  honours it — depends only on `bypass-core` + `uniffi` +
  `thiserror`.
- [ADR-0006](0006-trait-associated-error-types.md): the
  flat `BypassError` enum is a *flattening* of the
  generic `StoreError<C, S, V>` core uses internally.
  UniFFI can't represent the generics across the FFI; the
  flat enum is the natural projection.
- [ADR-0022](0022-native-messaging-wire-protocol.md): the
  *other* FFI surface (browser extension). Different
  consumer, different tooling, deliberately not unified.
