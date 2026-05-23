<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Changelog

All notable changes to `bypass` are recorded here. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); the
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

The `Unreleased` section accumulates work landed on `main` since
the last tagged release. Once `v0.1.0` ships it'll be cut into a
dated heading and the running notes will start a fresh
`Unreleased` block below it.

## [Unreleased]

The pre-release work that will become `v0.1.0`. Covers everything
between the initial commit (`affa4ed`, project scaffold) and the
post-eval CLI polish (`e507a43`). Three frontends now build green
on CI: the Linux CLI (Phases 1–6), the Firefox + Chrome browser
extension (Phase 7), and the Android app (Phase 8). See
[`doc/ROADMAP.md`](doc/ROADMAP.md) for the phase-by-phase plan
each item lives under.

### Added

#### Linux CLI (Phases 1–6)

- `bypass init <recipient...>` to seed a new pass-compatible
  store. Writes `.gpg-id`, initialises the git repo with the
  `*.gpg binary` `.gitattributes` rule
  ([ADR-0011](doc/adr/0011-sync-semantics-hybrid.md)), creates
  the store root at mode `0700`.
- CRUD: `bypass insert` (single-line + `-m` multiline; refuses
  zero-byte plaintext per CLI eval F2), `bypass show` (full
  entry or `<field>`; case-insensitive field matching),
  `bypass ls [<subpath>]` (ASCII tree, hides empty dirs),
  `bypass find <pattern>` (substring; exits 1 with a stderr
  message on no matches per CLI eval F5), `bypass rm`
  (`--recursive` for subtrees, shred-on-remove
  per [ADR-0008](doc/adr/0008-secure-delete-via-overwrite.md)),
  `bypass cp` / `bypass mv`, `bypass edit` (tempfile under
  `XDG_RUNTIME_DIR` + `$EDITOR` round-trip).
- `bypass generate <path> [length]` — `OsRng`-backed
  ([ADR-0007](doc/adr/0007-csprng-source.md)) password
  generation. `-n` for alphanumeric-only, `-i` to rotate the
  first line while preserving the body, `-f` to overwrite, `-c`
  to copy to clipboard.
- `bypass otp <path>` — current TOTP code for entries holding
  an `otpauth://` URI.
- `bypass log [<path>]` — git history, optionally narrowed to
  one entry.
- `bypass doctor` — tabular env probe (gpg version, secret
  keys, store root + perms, `.gpg-id` recipients in keyring,
  `$EDITOR`, git version, `.gitattributes` rule, audit
  cleanliness). Exits 1 if any row fails.
- `bypass audit` — scan unpushed commits for files that don't
  look like OpenPGP ciphertext or recognised metadata. Exits 1
  on findings.
- `bypass git <…>` passthrough — runs system `git` against the
  store's repo. Soft-warns before known-destructive shapes per
  CLI eval F6.
- `bypass ext <name>` — pass-compatible extension dispatcher
  (`<store>/.extensions/`, `$PASSWORD_STORE_EXTENSIONS_DIR/`,
  `~/.password-store-extensions/`).
- `bypass completion <shell>` — shell completion script
  generator (bash/zsh/fish/elvish/powershell).
- `bypass man` — emits the `bypass(1)` man page in groff.
- **Clipboard with auto-clear** — `-c` flag detaches a child
  process that owns the clipboard for ~45 s then restores the
  prior contents ([ADR-0019 §clipboard](doc/adr/)). Foreground
  probes `arboard::Clipboard::new()` before claiming success
  per CLI eval F7.
- Auto-commits per CRUD op with descriptive messages
  (`bypass: Add password for X`, `bypass: Rename A to B`,
  etc.).

#### Sync (Phase 5)

- Git-backed sync via any remote (`bypass sync`): `pull --rebase`
  + leak-check audit + `push`. Refuses to push when the audit
  finds non-`.gpg` files in the pending commits unless `--force`.
- LAN P2P sync over libp2p
  ([ADR-0010](doc/adr/0010-p2p-transport-libp2p.md)) — pack
  exchange between paired devices with mDNS discovery, SPAKE2
  pairing ([ADR-0012](doc/adr/0012-pake-spake2.md)) keyed on
  pinned peer identities (`peers.toml`), Ed25519 device
  identity ([ADR-0015](doc/adr/0015-device-identity-key.md)),
  symmetric-divergence resolution by peer-id lexical order
  ([ADR-0014](doc/adr/0014-sync-metadata-and-ordering.md)),
  per-peer attempt rate-limiter
  ([ADR-0016](doc/adr/0016-pairing-rate-limit.md)).
- `bypass sync daemon` — foreground long-runner watching the
  store for changes via `notify` + reconciling with paired
  peers.
- `bypass sync daemon install/uninstall/start/stop/enable/disable/status` —
  per-OS service supervision
  ([ADR-0020](doc/adr/0020-daemon-service-supervision.md)):
  systemd user unit on Linux, launchd agent on macOS.
- `bypass sync peer ls/add/rm` — pinned-peer management with
  revocation semantics
  ([ADR-0019](doc/adr/0019-peer-revocation-trust-semantics.md)).
- `bypass sync identity rotate` — rotate this device's identity
  key.

#### Browser extension (Phase 7)

- **Native messaging host** in the CLI: `bypass messaging-host`
  speaking length-prefixed JSON over stdin/stdout per
  [ADR-0022](doc/adr/0022-native-messaging-wire-protocol.md).
  Seven ops: `ls`, `find`, `show`, `insert`, `generate`,
  `otp`, `rm`. 512 KB reply cap; plaintext-carrying buffers
  in `Zeroizing<_>`.
- `bypass messaging-host install [--chrome-id <id>] [--firefox-id <id>]` /
  `uninstall` — writes / removes the per-browser
  native-messaging manifest at the Firefox + Chrome
  conventional paths on Linux + macOS.
- **MV3 extension** under [`extension/`](extension/) per
  [ADR-0023](doc/adr/0023-browser-extension-architecture.md).
  Single TypeScript codebase, esbuild + tsc, no runtime npm
  deps; vanilla DOM popup with debounced search + copy-to-
  clipboard. Promise-based JNA-equivalent native client at
  `extension/src/native.ts`; UniFFI-equivalent wire-format
  types mirrored in `extension/src/types.ts`.

#### Android app (Phase 8)

- `crates/bypass-ffi/` cdylib using UniFFI 0.29 in proc-macro
  mode per
  [ADR-0024](doc/adr/0024-android-ffi-via-uniffi.md). Concrete
  `BypassStore` wraps `Store<CryptoCallback, AppStorage,
  NoVcs>` (no git on Android in v0.1.0); `Crypto` is a
  callback interface Kotlin implements; `BypassError` flattens
  the parametrised core `StoreError<C, S, V>` for the FFI
  boundary. Slim in-crate `AppStorage` (no shred, no symlink
  rejection — rationale in source per single-tenant Android
  sandbox).
- Android Compose app under [`android/`](android/) per
  [ADR-0025](doc/adr/0025-android-ui-architecture.md). Single
  `MainActivity` + `NavHost`; five screens (`Init`, `List`,
  `Show`, `Insert`, `Generate`); manual DI via a
  `BypassApplication` singleton; Material 3 with dynamic
  colour on Android 12+.
- **OpenKeychain integration** via the
  `org.sufficientlysecure:openpgp-api:v11` JitPack artefact
  — binds OpenKeychain's `OPEN_PGP_SERVICE` AIDL endpoint;
  `AndroidManifest.xml` `<queries>` block for Android 11+
  package-visibility filtering.
- **Async PendingIntent bridge** (8.2.b.ii): `CryptoUiBridge`
  surfaces OpenKeychain user-interaction `PendingIntent`s as a
  `SharedFlow` the `MainActivity` collects on; the
  OpenKeychainCrypto IO thread blocks on a 1-slot
  `LinkedBlockingQueue` waiting for the `ActivityResult`. Re-
  executes the original `executeApi` call automatically on
  user confirmation. Capped at five interaction rounds per
  op so a misbehaving service can't loop. Verified on-device
  end-to-end (Nokia G50 / A13 / arm64-v8a).

#### CI / tooling

- Multi-OS test matrix on Linux + macOS ([`.github/workflows/ci.yml`](.github/workflows/ci.yml)).
- Supply-chain CI: `cargo-deny` (per `deny.toml`) + `cargo-audit`
  with a curated `--ignore` list for upstream-unfixed RUSTSEC
  advisories.
- Android cross-compile job (`android-ffi`): `cargo ndk -t
  arm64-v8a -t armeabi-v7a` against `bypass-ffi`.
- Android Gradle build job (`android-gradle-build`): full
  `./gradlew :app:assembleDebug` producing the debug APK as a
  workflow artefact; gated on a `changes` paths-filter so the
  7-min Gradle build only runs when `android/`,
  `crates/bypass-ffi/`, `crates/bypass-core/`, `Cargo.{toml,lock}`,
  or the workflow itself change.
- Extension typecheck + bundle smoke job
  (`extension-typecheck`): `tsc --noEmit` + `node build.mjs`.
- Release packaging on `v*` tags
  ([`.github/workflows/release.yml`](.github/workflows/release.yml)
  + [ADR-0021](doc/adr/0021-release-packaging.md)): hand-rolled,
  four Unix targets.

### Security

- All in-memory plaintext wraps in
  [`SecretBytes`](crates/bypass-core/src/crypto.rs) or
  `Zeroizing<_>`; auto-clear on drop.
- Store root mode `0700` on `init` and `doctor` so a sibling
  user on a multi-tenant box can't list entry names.
- Audit-before-push leak check refuses to publish non-`.gpg`
  files.
- `bypass init` refuses to overwrite an existing `.gpg-id`
  without `--force` (CLI eval F1) — guards against the
  store-splitting footgun where existing entries are
  encrypted to the old recipient while new inserts target a
  different key.
- `bypass insert` refuses zero-byte plaintext (CLI eval F2) —
  avoids creating "I forgot the password" entries that
  decrypt to nothing.
- `bypass git` passthrough soft-warns before destructive
  shapes (CLI eval F6).
- ICMP / mDNS surfaces only emit / accept frames over
  cryptographic identities; LAN broadcast is not used for
  trust.
- Browser extension manifest has no `externally_connectable`;
  the native-host manifest pins the extension id so a rogue
  local extension can't impersonate us.

### Notes / open work

Not yet shipped, tracked in [`doc/ROADMAP.md`](doc/ROADMAP.md):

- **7.2.b**: in-page autofill on user gesture; AMO / Chrome
  Web Store automation. The build script already emits a
  loadable / submittable zip; uploads are manual for now.
- **8.2.c**: optional libgit2-on-NDK for device-side sync.
  Currently the Android client uses `NoVcs`; cross-device
  sync goes through the desktop side of Phase 5.2.
- **Browser extension icon**: the manifest currently has no
  `icons` field; both browsers show a default puzzle-piece.
  Replace before any AMO / CWS submission.
- **Android icon**: placeholder vector "by" wordmark; replace
  before any Play Store submission.

[Unreleased]: https://github.com/hiroshiyui/bypass/compare/affa4ed...HEAD
