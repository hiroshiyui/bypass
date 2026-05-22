# bypass

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
| Linux CLI (`bypass`) | ✅ Phases 1–5.2 shipped (CRUD + git + generation + clipboard + structured fields + TOTP + extensions + sync + leak-check audit + LAN P2P sync) |
| Firefox & Chrome extension | 🗓 Planned (Phase 7) |
| Android app | 🗓 Planned (Phase 8) |

## Getting started

You need a Rust toolchain (edition 2024) and the system `gpg`
binary on your `$PATH`. `bypass doctor` will check both for you,
plus your store directory and recipients.

### Install (recommended)

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
- It watches the store for changes (via `inotify` / `FSEvents`) and
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
  multicast routing to work. On Linux check
  `ip route show table all | grep 224.0.0.0`. On macOS this
  generally just works. As a fallback, use git-backed sync over a
  shared remote.
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
