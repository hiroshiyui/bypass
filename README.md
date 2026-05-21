# bypass

> ⚠️ **Early development — not yet stable.**
> The Linux CLI is feature-complete for basic CRUD and git-backed
> versioning, but `bypass` has not been audited and has not cut a
> tagged release. Do not migrate a production password store to it
> yet. If you need a battle-tested password manager today, use
> [`pass`](https://www.passwordstore.org/); when `bypass` ships,
> your existing pass store will continue to work unchanged.

A password manager that keeps your secrets in plain files on disk, encrypted with your own OpenPGP key — compatible with [`pass`](https://www.passwordstore.org/), but built to follow you across devices.

## Why bypass?

- **Your keys, your files.** Every entry is an OpenPGP-encrypted file under `~/.password-store/`. No proprietary database, no vendor account, no cloud lock-in.
- **`pass`-compatible.** Already use `pass`? Point `bypass` at the same directory and it just works.
- **Sync the way you already sync.** Stores are git repositories. Push to your own server, GitHub, a USB stick, anything git can talk to.
- **One password store, every device.** A single Rust core powers the Linux command line today, with an Android app and Firefox/Chrome browser extensions planned next — all reading the *same* store, encrypted to the *same* keys.

## Status

`bypass` is **functional on Linux** for everyday password-manager use:
basic CRUD (`init`, `insert`, `show`, `ls`, `find`, `rm`, `edit`,
`cp`, `mv`), an environment doctor (`doctor`), and full git-backed
history (every mutation auto-commits, and `bypass git …` forwards
to your system `git`). Password generation, clipboard integration,
structured-field/OTP support, and the browser/Android frontends are
still ahead. Track progress in [`doc/ROADMAP.md`](doc/ROADMAP.md);
load-bearing design decisions live in
[`doc/adr/`](doc/adr/README.md).

| Frontend | Status |
|---|---|
| Linux CLI (`bypass`) | ✅ Phases 1–4 shipped (CRUD + git + generation + clipboard + structured fields + TOTP + extensions) |
| Firefox & Chrome extension | 🗓 Planned (Phase 7) |
| Android app | 🗓 Planned (Phase 8) |

## Getting started

You need a Rust toolchain (edition 2024) and the system `gpg` binary
on your `$PATH`. `bypass doctor` will check both for you, plus your
store directory and recipients.

### Install (recommended)

```sh
git clone https://github.com/hiroshiyui/bypass.git
cd bypass
cargo install --path crates/bypass-cli
bypass doctor
```

`cargo install` drops the release-mode `bypass` binary into
`~/.cargo/bin/`, which should already be on your `$PATH` if you
installed Rust via `rustup`. To upgrade later, `git pull` and re-run
the same `cargo install --path crates/bypass-cli` — it overwrites in
place. `cargo uninstall bypass` removes it. (Note: this is a workspace,
so `cargo install --path .` from the root won't work — point it at the
`bypass-cli` crate explicitly.)

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

Sync is just git — every store is a real git repository, and the
`bypass git` subcommand forwards arbitrary arguments to it:

```sh
bypass git remote add origin git@example.com:you/passwords.git
bypass git push -u origin main
bypass git log --oneline
```

Still to come: a `bypass sync` shortcut (Phase 5), and the browser /
Android frontends (Phases 7 & 8) — see
[`doc/ROADMAP.md`](doc/ROADMAP.md).

## Crypto, briefly

`bypass` never implements OpenPGP itself. Instead it asks whichever provider your platform already trusts:

- **Linux:** the `gpg` binary on your `$PATH`
- **Android:** [OpenKeychain](https://www.openkeychain.org/)
- **Browser extensions:** the `bypass` desktop binary, talking over the browser's native-messaging channel

This means your private key stays where you already keep it. `bypass` only ever sees ciphertext until you ask it to decrypt something.

## License

Licensed under the [GNU General Public License, version 3](LICENSE) or (at your option) any later version (`GPL-3.0-or-later`).
