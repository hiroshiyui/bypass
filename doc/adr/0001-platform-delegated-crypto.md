<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Platform-delegated OpenPGP crypto

* Status: accepted
* Date: 2026-05-21
* Deciders: hiroshiyui

## Context and Problem Statement

`bypass` must read and write pass-compatible OpenPGP-encrypted entries on
three very different platforms (Linux desktop, Android, browser). Each
platform has a strongly preferred — sometimes mandatory — way of dealing
with private keys:

* On Linux, users already have `gpg`/`gpg-agent` set up and expect their
  existing keyring, pinentry, and smartcard integrations to work.
* On Android, holding raw OpenPGP private keys inside a third-party app is
  poor hygiene; [OpenKeychain](https://www.openkeychain.org/) already
  exposes an OpenPGP AIDL service that hardware-backed keys can plug into.
* In the browser, an extension cannot reasonably hold long-lived private
  keys and cannot reach the desktop keyring directly.

We need to decide whether `bypass-core` speaks OpenPGP itself or delegates.

## Considered Options

* **Pure-Rust OpenPGP in core** (`sequoia-openpgp` or `rpgp`): one
  implementation, shared key handling.
* **Switch the storage format to `age`**: drop OpenPGP entirely; smaller
  crypto surface; loses pass compatibility.
* **Platform-delegated crypto**: core defines a `Crypto` trait; each
  frontend supplies an implementation backed by the platform's native
  OpenPGP provider.

## Decision Outcome

Chosen option: **platform-delegated crypto**, because:

* Existing pass users on Linux can point `bypass` at their current
  `~/.password-store` and it just works — no key migration, no second
  keyring, no second pinentry.
* On Android, private keys never enter the Rust process: OpenKeychain
  handles them in its own hardened storage and exposes a callback the Rust
  `Crypto` impl invokes via UniFFI. This is materially safer than bundling
  a key store inside `bypass`.
* The browser extension can stay tiny: it proxies requests to the desktop
  binary running as a native-messaging host, which already has a working
  `Crypto` impl. No web crypto, no key material in the browser process.
* `bypass-core` stays small, dependency-light, and easy to port (Android
  NDK, eventual `wasm32-unknown-unknown`). It never touches OpenPGP packets.

### Consequences

* Good: smallest possible crypto surface in the shared crate; per-platform
  trust models map onto first-class OS facilities.
* Good: a security bug in any one provider does not compromise the others.
* Bad: three providers to maintain, each with its own error surface (the
  `Crypto` trait carries an associated `Error` type — see
  [ADR-0006](0006-trait-associated-error-types.md)).
* Bad: the CLI's `gpg` subprocess path is slower per call than an in-process
  library would be. Acceptable for an interactive password manager.

### Confirmation

`bypass-core` MUST NOT depend on any OpenPGP, `age`, or generic-crypto
crate. Reviewers should reject any such addition to
`crates/bypass-core/Cargo.toml`. See also [ADR-0002](0002-pass-compatible-on-disk-layout.md)
for the storage format this decision is paired with.
