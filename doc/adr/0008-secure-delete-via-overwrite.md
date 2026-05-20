<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Secure-delete via overwrite in `StorageFs::remove`

* Status: accepted
* Date: 2026-05-21
* Deciders: hiroshiyui

## Context and Problem Statement

`bypass rm` deletes a password entry from disk. The naive
implementation — `unlink(2)` — leaves the file's contents intact in
free blocks until those blocks are reused, which is recoverable with
basic forensic tools on most desktop filesystems. For a tool whose
entire job is to keep secrets, that is a poor default.

`pass`, the project we are pass-compatible with
([ADR-0002](0002-pass-compatible-on-disk-layout.md)), does not
overwrite-before-unlink. We choose to do so anyway: it is a small
amount of code, it materially raises the bar against casual recovery,
and the cost is paid only when a user actively deletes an entry. The
question is *where* this behaviour lives.

## Considered Options

* **Trait method `Storage::secure_remove` with a default impl
  delegating to `remove`.** Every backend gets a chance to override.
  More explicit at the trait level.
* **Backend-local: shred lives inside `StorageFs::remove`.** The
  `Storage` trait stays unchanged. Other backends decide their own
  semantics.

## Decision Outcome

Chosen option: **backend-local shred inside `StorageFs::remove`**.

The `StorageFs` impl:

1. Opens the file `O_WRONLY`.
2. Writes [`SHRED_PASSES`] (=3) passes of cryptographic random bytes,
   matching GNU `shred(1)`'s default.
3. Calls `sync_all()` between passes to push each pass past the page
   cache before the next overwrites it.
4. Calls `fs::remove_file` to unlink.

Reasoning:

* "Securely delete a blob" is not a meaningful concept on every
  backend. Android app-scoped storage is encrypted at rest — once
  the app's data key is purged, any leftover bytes are
  cryptographically inaccessible, so overwrite-before-unlink is
  pointless overhead. An in-memory `Storage` (as used in unit tests)
  has nothing to overwrite. Forcing every backend to ship a
  `secure_remove` would propagate a leaky abstraction.
* The shred path needs filesystem-level primitives (`OpenOptions`,
  `seek`, `sync_all`) which `bypass-core` is forbidden from depending
  on by [ADR-0003](0003-workspace-split-core-cli.md).
* Keeping `Storage` minimal preserves the
  [ADR-0006](0006-trait-associated-error-types.md) trait-error
  discipline: the trait doesn't grow a third optional method whose
  semantics every reader has to remember.

### Consequences

* Good: tightest possible trait surface — backends opt in to secure-
  delete by *being a filesystem*.
* Good: `bypass edit` (Milestone 1.3, Commit 3) can reuse the same
  `overwrite_then_unlink` helper to wipe its tempfile, so the
  short-lived plaintext on disk shares the same secure-delete
  guarantee as a stored entry.
* Bad — limitations, copied from `shred(1)`'s caveats:
  * **Log-structured / copy-on-write filesystems** (btrfs, zfs, f2fs)
    do not overwrite in place. The shred passes write to *new*
    blocks, leaving the old blocks intact until the FS chooses to
    reclaim them.
  * **SSDs with wear-levelling** translate logical block writes to
    physical blocks at the firmware level. Three passes from
    userspace do not guarantee three passes over the same physical
    cells.
  * **Filesystem journals** (ext4 with `data=journal`) may capture a
    copy of the original contents in the journal before the
    overwrite even starts.
  * **Snapshots** (LVM, btrfs subvolumes, zfs, NAS) preserve the
    original blocks independently of the live filesystem.

  Users who need stronger at-rest guarantees should pair this with
  full-disk encryption (LUKS, FileVault) so that "the disk is off"
  is the recovery boundary, not "the file is unlinked".

### Confirmation

* Implementation: `crates/bypass-cli/src/storage_fs.rs` —
  `overwrite_then_unlink`, `overwrite_in_place`, `SHRED_PASSES`.
* Tests: `storage_fs::tests::remove_unlinks_the_file` and
  `storage_fs::tests::overwrite_in_place_replaces_contents`
  (the latter calls the overwrite step in isolation and asserts the
  bytes on disk no longer match the original).
* This ADR is referenced from the `storage_fs` module-level docs so
  the caveats above travel with the code.
