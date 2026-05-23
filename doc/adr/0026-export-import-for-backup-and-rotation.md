<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# `backup` / `restore` for backup, migration, and GPG key rotation

* Status: proposed
* Date: 2026-05-23
* Deciders: hiroshiyui

## Context and Problem Statement

The store layout we committed to in
[ADR-0002](0002-pass-compatible-on-disk-layout.md) ties every entry to
the recipient key(s) named in the nearest `.gpg-id`. That key is
expected to outlive the store, but in practice users rotate keys for
several reasons:

* Upgrading from an older RSA/DSA key to Ed25519 / ECC.
* Replacing a key that's been compromised, lost a subkey, or had its
  passphrase reset.
* Splitting personal and work recipients onto separate subtrees, or
  re-keying a shared subtree when a collaborator leaves.

Today `bypass init --force <new-key>` is the only knob, and
[`main.rs`](../../crates/bypass-cli/src/main.rs) deliberately refuses
to invoke it silently: overwriting `.gpg-id` alone leaves every
existing `*.gpg` blob encrypted to the *old* recipient while new
inserts target the new one — the store splits in two with no signal.
The user is left to script a decrypt-then-reinsert loop themselves,
which is awkward, error-prone, and leaves a window during which half
the tree is on the old key.

A related gap: `bypass` has no first-class **backup** story either.
Users currently lean on `git clone` of the store, which preserves
ciphertext but not the recipient identity, and does nothing for users
who haven't configured a git remote.

We want one design that covers both: a way to take the store's
contents out of the current `.gpg-id` regime and back into a (possibly
different) one, with no plaintext on disk and no half-rotated state.

## Considered Options

* **A. Bespoke `bypass reencrypt --to <new-key>` command.** Walks the
  store in place, decrypts each entry with the old recipient,
  re-encrypts to the new recipient, rewrites `.gpg-id`, commits.
  Single-purpose, easy to reason about.
* **B. Generic `export` (plaintext) + `import`.** Composable Unix-
  shaped tools. `export` emits plaintext (stdout); `import` ingests
  plaintext and re-encrypts to whatever `.gpg-id` the destination
  resolves. Rotation is `bypass export | bypass --store <new> import`.
* **C. Generic `backup --to <key>` + `restore`.** Like B, but
  the backup tarball is always wrapped in a single outer GPG
  encryption to a caller-specified key. Plaintext never leaves the
  process boundary except through `gpg --decrypt` during restore.
* **D. `backup` *and* `restore`, both in-place and to a new
  store.** Option C plus a `restore --in-place` mode that rewrites the
  *existing* git repo entry-by-entry instead of importing into a
  freshly-initialised store, preserving history and (importantly) not
  breaking the git ancestry shared with sync peers.

## Decision Outcome

Chosen option: **D — `backup` plus `restore` with both
`--in-place` and fresh-store modes; no plaintext `export`.**

The verbs `backup`/`restore` are deliberately distinct from the
foreign-format `import` introduced in
[ADR-0027](0027-foreign-format-importers.md): `backup`/`restore`
operate on bypass-native bundles (bypass → bypass), while `import`
ingests *foreign* vaults (Bitwarden, KeePass, …). One verb per
direction, with no overlap.

The surface:

```text
bypass backup --to <recipient> [--subtree <path>]  >  vault.tar.gpg
bypass restore vault.tar.gpg                       # into a fresh, init'd store
bypass restore --in-place vault.tar.gpg            # rewrite the current store
```

Semantics:

* `backup` decrypts every entry under `<subtree>` (default:
  store root) with the current `.gpg-id` recipients, packages the
  *plaintexts* plus a manifest (paths, mtimes, original recipient
  list) into a tar stream, and pipes that stream through `gpg
  --encrypt --recipient <recipient>` to stdout. The plaintext tar
  never touches disk: it streams through an OS pipe from our
  in-process tar writer into `gpg`'s stdin. `SecretBytes` wraps each
  decrypted blob between read and tar-write
  ([ADR-0001](0001-platform-delegated-crypto.md) keeps us out of the
  OpenPGP layer; we only own the plaintext in transit).
* For a **rotation**, `<recipient>` is the *new* key. The backed-up
  bundle is therefore already keyed to whatever `restore` will write
  back out — no double-decrypt, no re-encrypt step on restore beyond
  the per-entry one.
* `restore` decrypts the outer tarball with `gpg`, streams the inner
  tar through our reader, and for each entry calls
  `storage_fs::overwrite_then_unlink` on any prior file at that path
  before writing the freshly-encrypted blob
  ([ADR-0008](0008-secure-delete-via-overwrite.md) applies). In
  `--in-place` mode, the existing `.gpg-id` is rewritten *first* (so
  the per-entry encryption targets the new recipient), and the whole
  operation is wrapped in a single git commit:
  `Re-encrypt store for <new-key>`.
* In fresh-store mode, `restore` requires the target store to have
  been initialised (`bypass init <new-key>`) and to be empty. This
  preserves the "no surprise overwrites" rule from `main.rs:94-113`.

Reasoning:

* **No plaintext `export`.** It would be a useful primitive but
  shipping it as a documented command creates a footgun
  (`bypass export > vault.tar` on a shared filesystem is a disaster).
  The use cases that would want plaintext — piping into another
  password manager, archival to a non-GPG medium — are real but rare
  enough to defer; if we add them later, gating behind
  `--i-know-what-im-doing` and stdout-only is the form.
* **Inner re-encryption, not just outer wrapping.** A naïve
  `backup` could just tar the existing `*.gpg` files and
  wrap the tar — but the inner blobs would still be encrypted to the
  *old* key, so the bundle would be useless for rotation (whoever
  decrypts the tar still can't read the entries without the old
  private key). Making `backup` always re-encrypt the inner
  plaintexts to `--to` collapses backup and rotation into one
  primitive.
* **Two restore modes, not one.** Fresh-store restore is the cleanest
  semantically: atomic, no half-state, easy to reason about. But it
  discards git history and — critically for Phase 5.2 sync — breaks
  the shared git ancestry with paired peers
  ([ADR-0011](0011-sync-semantics-hybrid.md), [ADR-0014](0014-sync-metadata-and-ordering.md)).
  Peers would treat the restored store as a divergent history and
  refuse to fast-forward. `--in-place` preserves ancestry at the cost
  of one large rewrite commit, which peers *can* pull cleanly. Users
  rotating a solo store will reach for fresh-store; users on a
  multi-device sync mesh will reach for `--in-place`. Both are valid;
  picking only one would push the other group into hand-rolled
  scripts.
* **Crate placement.** Tar packing/unpacking and the manifest schema
  belong in `bypass-core` (pure logic, no I/O dependencies). Driving
  `gpg --encrypt`/`--decrypt` on the outer wrapper, spawning the
  pipe, and writing the git commit stay in `bypass-cli`
  ([ADR-0003](0003-workspace-split-core-cli.md)).

### Consequences

* Good: single mechanism covers three use cases (key rotation,
  off-site backup, store migration between machines).
* Good: rotation has no half-rotated intermediate state — either the
  new commit lands or it doesn't, and the old store is untouched
  until the new ciphertexts are written.
* Good: `backup` output is a useful artefact in its own
  right — a self-contained, GPG-sealed snapshot that doesn't depend
  on the rest of the bypass installation to restore from.
* Bad: plaintext lives in process memory (as `SecretBytes`) for the
  entire backup pass. For a large store this means N entries'
  plaintexts pass through RAM in sequence; we should stream one entry
  at a time through the tar writer rather than buffering the whole
  bundle, and rely on `SecretBytes`' zeroize-on-drop between entries.
* Bad: rotation does **not** retroactively protect ciphertext an
  attacker already exfiltrated. If the old private key leaks later,
  the old `*.gpg` blobs they captured are still readable. Rotation
  is forward-confidentiality only; users who need stronger
  guarantees must also roll the underlying *passwords*. This needs
  to be documented in the `bypass backup` help text.
* Bad: in `--in-place` mode, the rewrite commit is large (touches
  every blob) and will dominate the git history. Acceptable, but
  worth a heads-up in the help text.
* Bad: git history retains the old ciphertexts in prior commits.
  `git filter-repo` / BFG is the answer for users who need them
  scrubbed; bypass will not ship a history-rewriting command itself.
  Note this in the docs.
* Neutral: tar format choice (ustar vs pax) and manifest schema are
  implementation details, but the manifest *must* include a format
  version field so future restores can refuse incompatible bundles
  rather than mis-parse them.

### Confirmation

* Implementation lands under a new Phase 4 follow-up milestone in
  `doc/ROADMAP.md`; the milestone's checkbox is the confirmation that
  the design described here was executed.
* Tests live in `crates/bypass-cli/tests/end_to_end.rs` (default
  suite): a round-trip test that initialises a store with key A,
  inserts entries, runs `backup --to B`, restores into a
  fresh store initialised with key B, and asserts every entry
  decrypts with the same plaintext. A second test exercises
  `--in-place` and asserts the git log shows a single rewrite commit
  with the prior commits' ancestry intact.
* `bypass doctor` gains a check that warns if the store's `.gpg-id`
  names a key whose primary algorithm is RSA-1024 or DSA, nudging
  users toward rotation. (Stretch; not required for this ADR's
  acceptance.)
