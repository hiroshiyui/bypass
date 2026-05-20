# bypass

> ⚠️ **Work in progress — not usable yet.**
> `bypass` is in early development. There is no working binary, no released version, and no commands to run. Nothing below describes software you can use today; it describes what the project is being built to become. Do **not** trust your passwords to it. If you need a password manager right now, use [`pass`](https://www.passwordstore.org/) — when `bypass` ships, your existing pass store will continue to work.

A password manager that keeps your secrets in plain files on disk, encrypted with your own OpenPGP key — compatible with [`pass`](https://www.passwordstore.org/), but built to follow you across devices.

## Why bypass?

- **Your keys, your files.** Every entry is an OpenPGP-encrypted file under `~/.password-store/`. No proprietary database, no vendor account, no cloud lock-in.
- **`pass`-compatible.** Already use `pass`? Point `bypass` at the same directory and it just works.
- **Sync the way you already sync.** Stores are git repositories. Push to your own server, GitHub, a USB stick, anything git can talk to.
- **One password store, every device.** A single Rust core powers the Linux command line today, with an Android app and Firefox/Chrome browser extensions planned next — all reading the *same* store, encrypted to the *same* keys.

## Where things stand

`bypass` is in **early development**. The architecture is in place but the user-facing commands are still being built. Track progress in [`doc/ROADMAP.md`](doc/ROADMAP.md). The shipping plan:

| Frontend | Status |
|---|---|
| Linux CLI (`bypass`) | 🚧 In progress |
| Firefox & Chrome extension | 🗓 Planned (Phase 7) |
| Android app | 🗓 Planned (Phase 8) |

If you need a working password manager *today*, use [`pass`](https://www.passwordstore.org/). When `bypass` ships, your existing pass store will continue to work unchanged.

## How it will work *(planned — none of these commands work yet)*

Once the Linux CLI is functional, typical use will look like:

```sh
# Set up a store encrypted to your GPG key
bypass init you@example.com

# Add a password
bypass insert github.com/you

# Look it up
bypass show github.com/you

# Copy it to the clipboard (auto-clears after 45 seconds)
bypass show -c github.com/you

# Generate a strong password
bypass generate github.com/you 32

# Browse your store
bypass ls
bypass find github
```

Sync is just git:

```sh
bypass git remote add origin git@example.com:you/passwords.git
bypass git push
```

## Crypto, briefly

`bypass` never implements OpenPGP itself. Instead it asks whichever provider your platform already trusts:

- **Linux:** the `gpg` binary on your `$PATH`
- **Android:** [OpenKeychain](https://www.openkeychain.org/)
- **Browser extensions:** the `bypass` desktop binary, talking over the browser's native-messaging channel

This means your private key stays where you already keep it. `bypass` only ever sees ciphertext until you ask it to decrypt something.

## License

Licensed under the [GNU General Public License, version 3](LICENSE) or (at your option) any later version (`GPL-3.0-or-later`).
