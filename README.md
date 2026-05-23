# bypass

[![CI](https://github.com/hiroshiyui/bypass/actions/workflows/ci.yml/badge.svg)](https://github.com/hiroshiyui/bypass/actions/workflows/ci.yml)

> ⚠️ **Pre-release — not yet audited.**
> The Linux CLI now covers every feature in Phases 1–5.2 (CRUD, git,
> generation, clipboard, OTP, extensions, leak-check audit, LAN P2P
> sync), but `bypass` has not been audited and has not cut a tagged
> release. Don't migrate a production password store to it yet. If
> you need a battle-tested password manager today, use
> [`pass`](https://www.passwordstore.org/); when `bypass` ships your
> existing pass store will continue to work unchanged.

A password manager that keeps your secrets in plain files on disk,
encrypted with your own OpenPGP key — compatible with
[`pass`](https://www.passwordstore.org/), but built to follow you
across devices.

## Why bypass?

- **Your keys, your files.** Every entry is an OpenPGP-encrypted
  file under `~/.password-store/`. No proprietary database, no
  vendor account, no cloud lock-in.
- **`pass`-compatible.** Already use `pass`? Point `bypass` at the
  same directory and it just works — same on-disk layout, same
  `.gpg-id` recipient resolution.
- **Sync the way you already sync.** Stores are git repositories.
  Push to your own server, GitHub, a USB stick, anything git can
  talk to.
- **Or skip the server entirely.** Pair two devices on the same LAN
  with a 6-digit PIN; the [`bypass sync daemon`](#lan-peer-to-peer-sync)
  pushes commits to your other devices over libp2p with no
  intermediate server. (Phase 5.2 — see ADRs
  [0010](doc/adr/0010-p2p-transport-libp2p.md)–[0019](doc/adr/0019-peer-revocation-trust-semantics.md).)
- **One password store, every device.** A single Rust core powers
  the Linux command line today, with an Android app and
  Firefox/Chrome browser extensions planned next — all reading the
  *same* store, encrypted to the *same* keys.

## Status

`bypass` is **functional on Linux** for everyday password-manager
use. Track progress in [`doc/ROADMAP.md`](doc/ROADMAP.md);
load-bearing design decisions live in
[`doc/adr/`](doc/adr/README.md).

| Frontend | Status |
|---|---|
| Linux CLI (`bypass`) | ✅ Phases 1–6 shipped (CRUD + git + generation + clipboard + structured fields + TOTP + extensions + LAN P2P sync + sync daemon + CI/release packaging) |
| Firefox & Chrome extension | ✅ Phase 7 MVP shipped (popup + native messaging); 🗓 autofill (7.2.b) |
| Android library (`bypass-ffi`) | ✅ Phase 8.1 shipped (UniFFI crate + Android CI cross-compile) |
| Android app | ✅ Phase 8.2.b.ii shipped (Compose UI + OpenKeychain AIDL client with async PendingIntent bridge); 🗓 libgit2-on-NDK sync (8.2.c, optional) |

## Getting started

You need a Rust toolchain (edition 2024) and the system `gpg`
binary on your `$PATH`. `bypass doctor` will check both for you,
plus your store directory and recipients.

### Install (recommended)

If a release tagged `vX.Y.Z` exists on
[GitHub Releases](https://github.com/hiroshiyui/bypass/releases),
download the tarball for your platform, verify against
`SHA256SUMS`, and drop the binary on your `$PATH`:

```sh
# Example: Linux x86_64
TARBALL=bypass-v0.1.0-x86_64-unknown-linux-gnu.tar.gz
curl -fLO https://github.com/hiroshiyui/bypass/releases/latest/download/${TARBALL}
curl -fLO https://github.com/hiroshiyui/bypass/releases/latest/download/SHA256SUMS
sha256sum -c --ignore-missing SHA256SUMS
tar xzf "${TARBALL}"
install bypass-v0.1.0-x86_64-unknown-linux-gnu/bypass ~/.local/bin/
bypass doctor
```

Or build from source — works on every Phase 6 target:

```sh
git clone https://github.com/hiroshiyui/bypass.git
cd bypass
cargo install --path crates/bypass-cli
bypass doctor
```

`cargo install` drops the release-mode `bypass` binary into
`~/.cargo/bin/`, which should already be on your `$PATH` if you
installed Rust via `rustup`. To upgrade later, `git pull` and
re-run the same `cargo install --path crates/bypass-cli` — it
overwrites in place. `cargo uninstall bypass` removes it. (Note:
this is a workspace, so `cargo install --path .` from the root
won't work — point it at the `bypass-cli` crate explicitly.)

### Build from source without installing

If you'd rather run the binary in place — handy when hacking on
`bypass` itself — build the workspace and invoke it via cargo:

```sh
cargo build --release
./target/release/bypass doctor
# or
cargo run -p bypass --release -- doctor
```

## Usage

```sh
# Set up a store encrypted to your GPG key (creates a git repo too)
bypass init you@example.com

# Add a password (interactive: prompts twice with echo off)
bypass insert github.com/you

# Or pipe one in from a script
echo "hunter2" | bypass insert -- github.com/you

# Look it up
bypass show github.com/you

# Browse your store
bypass ls
bypass ls email           # scoped to a subtree
bypass find github        # substring search

# Copy / move entries (re-encrypts when crossing a .gpg-id boundary)
bypass cp  github.com/you  archive/github
bypass mv  archive/github  archive/github-old

# Edit an entry in $EDITOR (vi if unset); plaintext is staged to
# /dev/shm when available so it never hits permanent storage
bypass edit github.com/you

# Generate a strong random password (default 25 chars, alphanumeric + symbols)
bypass generate github.com/you 32

# Generate and copy to the clipboard, auto-clearing after ~45 s
bypass generate -c github.com/you

# Copy an existing entry to the clipboard instead of printing it
bypass show -c github.com/you

# Securely delete (shred-style overwrite before unlink — see ADR-0008)
bypass rm github.com/you

# Inspect history
bypass log github.com/you      # commits touching this entry
bypass log                     # full store history

# Multi-line entries: first line is the password, then `key: value` pairs
echo "hunter2
login: alice
url: https://example.com
otpauth://totp/Example:alice?secret=JBSWY3DPEHPK3PXP&issuer=Example" \
    | bypass insert -m service

# Show one field
bypass show service login                 # → alice
bypass show -c service login              # copy field value to clipboard

# Compute TOTP from the otpauth:// URI in the entry
bypass otp service
bypass otp -c service

# Run a pass-style extension
bypass ext my-extension --some-flag
```

### Shell integration

`bypass` emits shell completion scripts and a `bypass(1)` man page
on demand — redirect each to wherever your distribution expects
the file:

```sh
# Shell completions (pick the one for your shell)
bypass completion bash       > ~/.local/share/bash-completion/completions/bypass
bypass completion zsh        > ~/.zsh/completion/_bypass
bypass completion fish       > ~/.config/fish/completions/bypass.fish
bypass completion powershell > $PROFILE.CurrentUserAllHosts
bypass completion elvish     > ~/.config/elvish/lib/bypass.elv

# Man page (system-wide; needs sudo on most distros)
bypass man | sudo tee /usr/local/share/man/man1/bypass.1 > /dev/null
sudo mandb     # or `makewhatis`, depending on the distro
man bypass
```

## Syncing

`bypass` has **two** ways to keep multiple devices in sync. They
work together — most users want both.

### Git-backed sync (any remote)

Every store is a git repository. The `bypass git` subcommand
forwards arbitrary arguments to your system `git`:

```sh
bypass git remote add origin git@example.com:you/passwords.git
bypass git push -u origin main
bypass git log --oneline
```

Once a remote is configured, `bypass sync` is a one-shot
`git pull --rebase` + `git push`:

```sh
bypass sync                # pull --rebase, then push
bypass sync --force        # skip the leak-check audit (see below)
```

This works over SSH, HTTPS, a USB stick, anything git speaks. It's
the right answer when your devices are on different networks.

### LAN peer-to-peer sync

When two of your devices are on the same LAN, you can skip the
server entirely. `bypass sync daemon` holds a libp2p Swarm
([ADR-0010](doc/adr/0010-p2p-transport-libp2p.md)), watches the
store for changes, and pushes commits to paired peers as a git
pack ([ADR-0011](doc/adr/0011-sync-semantics-hybrid.md)).

#### One-time: pair two devices

On device A, generate a PIN and listen:

```sh
bypass sync pair --show
# PAIRING PIN: 528491
# Multiaddrs to share with the other device:
#   /ip4/192.168.1.42/tcp/45678/p2p/12D3KooW…
# waiting for the other device…
```

On device B, dial that multiaddr and enter the PIN:

```sh
bypass sync pair --enter --addr /ip4/192.168.1.42/tcp/45678/p2p/12D3KooW…
# Enter PIN from other device: 528491
# paired with device-a (12D3KooW…)
```

Pairing uses SPAKE2 ([ADR-0012](doc/adr/0012-pake-spake2.md)): the
PIN is single-use, expires after 5 minutes, and never crosses the
wire in cleartext. Both devices write a [`peers.toml`] record
pinning the other's libp2p identity.

#### Day-to-day: run the daemon

```sh
bypass sync daemon &
bypass sync status

# Daemon:    12D3KooW…abc
# Listening: /ip4/192.168.1.42/tcp/45678
# Peers:
#   phone     12D3KooW…xyz   discovered=yes   last=FastForwarded (2m ago)
#   laptop    12D3KooW…def   discovered=no    last=(never)
```

With the daemon running:

- It serves inbound `WantPackFrom` requests from paired peers
  (rate-limited per peer per
  [ADR-0016](doc/adr/0016-sync-dos-defences.md): 3 attempts / 60 s,
  pack-size cap 50 MB).
- It watches the store for changes (via `inotify`) and
  pushes the new history to every paired peer it can reach.
- It auto-discovers paired peers on the LAN via mDNS and dials
  them. (Requires the host's IPv4 multicast routes to be sane —
  on Linux this is the default.)
- Diverged histories are auto-resolved by rebase with a custom
  merge driver that takes the incoming side for opaque-ciphertext
  conflicts ([ADR-0011](doc/adr/0011-sync-semantics-hybrid.md)),
  with a peer-ID lexical tie-breaker
  ([ADR-0014](doc/adr/0014-sync-metadata-and-ordering.md)).

`bypass sync status --json` emits the same snapshot in JSON for
scripts and dashboards.

#### Run as a service (systemd)

`bypass sync daemon` runs in the foreground by default — handy
for evaluation, not great for "always-on". The
[ADR-0020](doc/adr/0020-daemon-service-supervision.md) service
ops install a systemd user unit at the conventional per-user path
(`~/.config/systemd/user/bypass-sync.service`), with the
`bypass` binary's current path baked in:

```sh
bypass sync daemon install   # write the unit
bypass sync daemon start     # run it now (this session only)
bypass sync daemon enable    # auto-start on every login

bypass sync daemon status    # ask the supervisor: is it running?
bypass sync status           # ask the running daemon: what peers do you see?

bypass sync daemon disable   # stop auto-starting
bypass sync daemon stop      # stop it now
bypass sync daemon uninstall # remove the unit / plist
```

`install` is off-by-default — it writes the file but does **not**
start the daemon or enable autostart. The user opts in explicitly
with `start` / `enable`. Re-run `install` after upgrading
`bypass` so the supervisor sees the new binary path.

#### Revoking a paired peer

```sh
bypass sync peer rm phone
# (prints what revocation does and does not cover, then exits 2)
bypass sync peer rm phone --yes
```

[ADR-0019](doc/adr/0019-peer-revocation-trust-semantics.md):
removing a peer prevents future syncs but does **not** rewrite
your git history. Commits authored by the revoked peer remain in
the repo. If you need a clean history, re-clone from a trusted
source or use `git filter-repo`.

#### Identity-key rotation

If you lose control of a device entirely (not just want to untrust
one paired peer), rotate this device's identity key — that
invalidates every existing pairing in one move:

```sh
bypass sync identity rotate --confirm
# Rotated identity. New peer id: 12D3KooW…
# Cleared 3 paired peer(s); re-pair every device with `bypass sync pair`.
```

The new identity is at
`$XDG_CONFIG_HOME/bypass/identity.key` with `0600` perms
([ADR-0015](doc/adr/0015-device-identity-key.md)).

### Safety net: leak-check audit

Before pushing (`git push` or peer-to-peer pack send), `bypass`
runs a quick audit over the commits about to be published and
**refuses to push if anything doesn't look like OpenPGP
ciphertext** — say, a stray editor swap file (`.work.gpg.swp`), a
`.gpg` file that's actually plaintext, or an unexpected file like
`notes.txt`. The same check is available standalone as
`bypass audit` and shows up as a row in `bypass doctor`. See
[ADR-0009](doc/adr/0009-leak-check-before-push.md) for the full
rationale.

```sh
bypass audit       # exit 0 if clean; exit 1 with a list of issues
bypass sync        # refuses on any issue
bypass sync --force  # explicit override
```

If you intentionally commit a non-`.gpg` file (e.g. a `README.md`
that travels with the store), `audit` allows it; it only flags
files outside the recognised allowlist.

### Conflict resolution

Encrypted `.gpg` files can't be auto-merged by git — it sees them
as binary. The 5.2.b custom merge driver
([`bypass-take-theirs`](doc/adr/0011-sync-semantics-hybrid.md))
resolves `.gpg` conflicts during peer-to-peer auto-rebase by
taking the incoming side. For git-remote sync (`bypass sync`),
where the merge driver isn't auto-invoked, you have three
choices when a rebase fails:

1. **Take theirs** (overwrite local with remote):
   ```sh
   bypass git checkout --theirs <path>.gpg
   bypass git add <path>.gpg
   bypass git rebase --continue
   ```
2. **Take mine** (keep local, throw remote away):
   ```sh
   bypass git checkout --ours <path>.gpg
   bypass git add <path>.gpg
   bypass git rebase --continue
   ```
3. **Hand-merge**: `bypass edit <entry>` to merge the plaintexts
   manually, `bypass git add`, then `bypass git rebase --continue`.

Last resort: `bypass git rebase --abort` to bail out, decide on a
strategy, then re-attempt `bypass sync`.

## Backing up and rotating keys

`bypass backup --to <recipient>` streams a GPG-wrapped tar of your
decrypted store to stdout — the bundle is sealed under any recipient
you name, so the same primitive covers off-site backup and GPG key
rotation (see [ADR-0026](doc/adr/0026-export-import-for-backup-and-rotation.md)).
Plaintext never touches disk: the tar bytes pipe straight into
`gpg --encrypt`'s stdin.

### Backup to a different machine

```sh
bypass backup --to backup@example > vault.tar.gpg
# copy vault.tar.gpg somewhere — USB stick, cloud storage, another host
```

On the recovery machine (with `backup@example`'s private key in the
local keyring):

```sh
mkdir -p ~/.password-store-recovered
PASSWORD_STORE_DIR=~/.password-store-recovered bypass init backup@example
PASSWORD_STORE_DIR=~/.password-store-recovered bypass restore vault.tar.gpg
```

### Rotating to a stronger GPG key

Two flavours. Pick **fresh-store** if you want a clean new store and
don't need the existing git history (solo machine; nothing paired
yet). Pick **in-place** if your store is already on a sync mesh —
the rewrite lands as a single commit so paired peers can fast-forward
without ancestry breakage.

Fresh-store:

```sh
bypass backup --to new-key@me > vault.tar.gpg
# In a fresh tempdir or after moving the old store aside:
PASSWORD_STORE_DIR=~/.password-store-new bypass init new-key@me
PASSWORD_STORE_DIR=~/.password-store-new bypass restore vault.tar.gpg
# When you're satisfied, replace the old store.
```

In-place (preserves history; peers can pull cleanly):

```sh
bypass backup --to new-key@me > vault.tar.gpg
# Swap the recipient:
echo new-key@me > ~/.password-store/.gpg-id
bypass restore --in-place vault.tar.gpg
bypass git log --oneline | head -3   # → single `Re-encrypt store for new-key@me` on top
```

### Caveats

* **Rotation is forward-confidentiality only.** Old ciphertext blobs
  an attacker exfiltrated *before* the rotation are still readable
  if the old private key ever leaks. If you suspect a particular
  password may have been captured, rotate the *password* — not just
  the GPG key.
* **Git history retains the old ciphertexts.** Prior commits contain
  blobs encrypted to the old recipient. `git filter-repo` (or BFG)
  is the user's tool for scrubbing them; `bypass` deliberately does
  not ship a history-rewriting command.
* **`backup` / `restore` carry bypass-native bundles only.** For
  foreign vaults (Bitwarden, KeePass, LastPass, …) the design is
  `bypass import` per [ADR-0027](doc/adr/0027-foreign-format-importers.md);
  see the next section.

## Importing from another password manager

`bypass import --format=<name>` ingests a foreign vault into the
current store. The verb is deliberately distinct from
`backup`/`restore` (which move bypass-native bundles only): `import`
is one-way, foreign → bypass.

First-party formats: **Bitwarden** plain-JSON, **generic CSV**.
Anything else routes through a `bypass-import-<name>` extension
(see [ADR-0027](doc/adr/0027-foreign-format-importers.md);
extension dispatch surface lands later in Milestone 4.5).

### Bitwarden

In the Bitwarden web vault / desktop / mobile, run **Tools → Export
Vault** with the file format set to `.json` and the file password
left **empty** — encrypted exports are not yet supported.

```sh
bypass import --format=bitwarden ~/Downloads/bitwarden_export.json
```

Logins, secure notes, custom fields, TOTP URIs, and free-form notes
all carry through. Card and identity items are skipped with a
stderr "lossiness" line — they don't map cleanly to a single-
password entry.

### CSV

Because every password manager exports CSV with a different column
layout, you state your own:

```sh
bypass import \
    --format=csv \
    --csv-schema=name,username,password,url,notes \
    --csv-has-header \
    vault.csv
```

Role names you can pass: `name`, `folder`, `password`, `username`,
`url`, `totp`, `notes`, `-` (skip a column). Anything else becomes a
custom field with that header name. Multiple `url` columns become
`url`, `url-2`, `url-3`, ….

### What survives the import

`bypass` prints a stderr summary at the end of every import that
names every field it dropped, transformed (e.g. embedded newlines
flattened), or had to disambiguate (in-batch path collisions are
suffixed `-2`, `-3`, …). Read it before deleting the source vault.

If the import would collide with an entry already in the destination
store, `bypass` refuses atomically — none of the import is written —
and prints the full list of conflicting paths. Either `rm` the
prior entries or merge by hand.

## Browser extension (Firefox + Chrome)

`bypass` ships a Manifest V3 WebExtension under
[`extension/`](extension/) that delegates every privileged
operation back to the desktop binary via the browser's
[native messaging](https://developer.chrome.com/docs/extensions/develop/concepts/native-messaging)
channel. The extension is a thin UI: search the store, click
to copy a password. All crypto / git / store I/O stays on the
desktop side, behind the same `gpg` subprocess the CLI has
always used.

- **Wire protocol**: [ADR-0022](doc/adr/0022-native-messaging-wire-protocol.md)
- **Extension architecture**: [ADR-0023](doc/adr/0023-browser-extension-architecture.md)
- **Browser support**: Firefox + Chrome. Chromium / Brave /
  Edge / Vivaldi load the same artefact via Chrome's manifest
  path; see [`extension/README.md`](extension/README.md).

```sh
# 1. Register the native host (run once per machine).
#    Re-run with --chrome-id <id> after loading the extension
#    unpacked, using the ID shown on chrome://extensions.
bypass messaging-host install

# 2. Build the extension.
cd extension/
npm ci
node build.mjs              # writes extension/dist/
node build.mjs --zip        # also writes bypass-extension-X.Y.Z.zip

# 3. Load unpacked.
#    Firefox: about:debugging → "Load Temporary Add-on" → dist/manifest.json
#    Chrome:  chrome://extensions → Developer mode → Load unpacked → dist/
```

Removing the host manifests:

```sh
bypass messaging-host uninstall
```

Out of scope for v1: in-page autofill (Phase 7.2.b), icons
(both browsers fall back to a default puzzle-piece), and
AMO / Chrome Web Store submission automation. See
[`extension/README.md`](extension/README.md) for v1
limitations and troubleshooting.

## Android library (FFI crate)

[`crates/bypass-ffi/`](crates/bypass-ffi/) is the Rust FFI
surface for the planned Android app — a `cdylib` that
[UniFFI](https://mozilla.github.io/uniffi-rs/) wraps for
Kotlin / Swift consumers. See
[ADR-0024](doc/adr/0024-android-ffi-via-uniffi.md) for the
wire shape; the Kotlin surface looks like:

```kotlin
val store = BypassStore.open(
    rootDir = context.filesDir.resolve("store").absolutePath,
    crypto  = OpenKeychainCrypto(context),  // implements `Crypto`
)
store.init(listOf("you@example.com"))
val plaintext: ByteArray = store.show("email/work")
```

`Crypto` is a UniFFI callback interface: Kotlin implements it
against
[OpenKeychain](https://github.com/open-keychain/open-keychain)'s
OpenPGP AIDL service so the user's existing keyring stays in
OpenKeychain — same platform-delegated-crypto posture
[ADR-0001](doc/adr/0001-platform-delegated-crypto.md) sets
for every frontend.

Phase 8.1 ships the Rust crate + CI cross-compile for
`aarch64-linux-android` and `armv7-linux-androideabi`.
Phase 8.2.a builds the Compose app on top (see below).

```sh
# Local exploration (NDK required for the cross-compile):
rustup target add aarch64-linux-android armv7-linux-androideabi
cargo install --locked cargo-ndk
cargo ndk -t arm64-v8a -t armeabi-v7a build --release -p bypass-ffi

# Emit Kotlin bindings to a tempdir (the Android Gradle build
# under android/ runs this as a build step):
cargo run -p bypass-ffi --bin uniffi-bindgen -- \
    generate \
    --library target/debug/libbypass.so \
    --language kotlin \
    --out-dir /tmp/bypass-kotlin
```

## Android app

[`android/`](android/) hosts the Gradle / Compose / Kotlin
app on top of the FFI crate. Manifest V… well, Manifest is
Android's own thing — single `MainActivity` + Compose
NavHost + per-screen `ViewModel`, manual DI, Material 3
theming with dynamic colour on Android 12+. Locked in by
[ADR-0025](doc/adr/0025-android-ui-architecture.md).

**Status**: Phase 8.2.b.ii shipped. Compose UI on the device,
binding to OpenKeychain's OpenPGP AIDL service for every
encrypt / decrypt — same platform-delegated-crypto posture
[ADR-0001](doc/adr/0001-platform-delegated-crypto.md) sets
for the desktop CLI's `gpg` subprocess. Install
[OpenKeychain](https://github.com/open-keychain/open-keychain)
on the device, add at least one OpenPGP key to its keyring,
then bypass works end-to-end. When OpenKeychain needs the
user to unlock (cold passphrase cache, key picker), the
8.2.b.ii async `PendingIntent` bridge auto-launches its UI
and resumes the encrypt / decrypt call when the user
confirms — no manual retry.

```sh
# Open `android/` in Android Studio (Iguana or later).
# Or from the CLI, once you have cargo-ndk + the Android NDK:
cd android/
./gradlew assembleDebug              # builds the debug APK
./gradlew installDebug               # installs to a connected device / emulator
```

See [`android/README.md`](android/README.md) for the full
walk-through, including the OpenKeychain prereqs, the Rust ↔
Kotlin Gradle integration, and what 8.2.c would add (optional
libgit2-on-NDK device-side sync).

## Migrating from `pass`

The on-disk format is identical to
[`pass`](https://www.passwordstore.org/)
([ADR-0002](doc/adr/0002-pass-compatible-on-disk-layout.md)):
`<store>/<entry>.gpg` for every secret, `.gpg-id` files at the
root and per-subtree to declare recipients, and a regular git
repository on top. Migrating is therefore a no-op — just point
`bypass` at your existing store:

```sh
# Either set the env var (matches pass's own convention)
export PASSWORD_STORE_DIR=~/.password-store

# …or accept the default ~/.password-store and don't set anything.
bypass doctor      # confirms gpg, recipients, .gpg-id, .gitattributes
bypass ls          # browse what's already there
bypass show github.com/you
```

If your store predates `bypass init`'s `.gitattributes` auto-write
(any store created by `pass` will), running `bypass sync` once
installs the canonical `*.gpg binary merge=bypass-take-theirs`
rule into `.gitattributes` and commits it — see the message
"installed missing `.gitattributes` rule" the first time. From
then on, `bypass` and `pass` can share the same store byte-for-byte.

## Troubleshooting

`bypass doctor` is the first stop for any "why doesn't this work"
question. It checks (in order):

- `gpg` is on `$PATH` and works
- `GNUPGHOME` is sane
- The store directory exists and is writable
- `.gpg-id` is present and lists at least one recipient
- Every recipient resolves to a secret key
- `$EDITOR` is set or `vi` is installed
- `git` is available
- The store has a `.gitattributes` rule for `*.gpg` (auto-installed
  by `bypass init`; lazily upgraded by `bypass sync` on legacy
  stores)

Common pitfalls:

- **"daemon not running"** when running `bypass sync status` —
  start it: `bypass sync daemon &`. The status command only talks
  to a live daemon; the one-shot `bypass sync` doesn't need one.
- **Pairing fails with "wrong PIN"** — PINs expire 5 minutes
  after `--show` displays them
  ([ADR-0012](doc/adr/0012-pake-spake2.md)); generate a fresh
  one. Three failed attempts in 60 s per peer trigger the
  ADR-0016 rate limit; wait it out.
- **Peer never appears as `discovered: yes`** — mDNS needs
  multicast routing to work. Check
  `ip route show table all | grep 224.0.0.0`. As a fallback, use
  git-backed sync over a shared remote.
- **`bypass sync` refuses to push** — the leak-check audit found
  something non-ciphertext. Run `bypass audit` to see what; fix
  it locally; re-run. Use `--force` only if you're sure.

## Crypto, briefly

`bypass` never implements OpenPGP itself. Instead it asks
whichever provider your platform already trusts:

- **Linux:** the `gpg` binary on your `$PATH`
- **Android:** [OpenKeychain](https://www.openkeychain.org/) (Phase 8)
- **Browser extensions:** the `bypass` desktop binary, talking over
  the browser's native-messaging channel (Phase 7)

This means your private key stays where you already keep it.
`bypass` only ever sees ciphertext until you ask it to decrypt
something.

## Acknowledgements

`bypass` is a re-implementation of, and intends to remain
on-disk-compatible with, [`pass`](https://www.passwordstore.org/)
by Jason A. Donenfeld. If `pass` works for you today, stick with
it; `bypass` is for folks who want the same on-disk format with
multi-device sync (Phase 5.2), an Android app (Phase 8), and a
browser extension (Phase 7) maintained from one codebase.

## License

Licensed under the [GNU General Public License, version 3](LICENSE)
or (at your option) any later version (`GPL-3.0-or-later`).
