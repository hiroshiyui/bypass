<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Phase 5.2 — LAN P2P sync: design evaluation

> Status: design doc, no implementation yet. Locks in the decisions
> Milestone 5.2's sub-milestones will build on. The canonical record
> of each decision lives in its ADR:
> [0010 transport](adr/0010-p2p-transport-libp2p.md),
> [0011 sync semantics](adr/0011-sync-semantics-hybrid.md),
> [0012 PAKE](adr/0012-pake-spake2.md),
> [0013 sync test strategy](adr/0013-sync-transport-trait.md).
> Where this doc and an ADR disagree, the ADR wins.

## Context

[Milestone 5.1](ROADMAP.md) shipped `bypass sync`, which runs
`git pull --rebase` + `git push` through whatever git remote the
user configured. That's adequate for the "one remote, deliberate
sync" workflow but misses two scenarios real password-manager users
hit:

1. **Multiple devices on the same LAN, no shared remote.** Phone
   and laptop in the same flat; no GitHub account, no SSH server.
2. **Background sync.** The user wants the laptop and phone to
   stay in agreement without running a command every time, the
   way Syncthing or Dropbox would.

Phase 5.2 is the roadmap entry for "LAN P2P sync (stretch)". This
document weighs the design space and records the decisions that
the implementation sub-milestones will be built on. It does *not*
plan implementation; that lives in subsequent planning sessions,
one per sub-milestone.

## Goals

- **Device-to-device sync on a LAN** with no required cloud service.
- **Multiple devices stay in agreement** automatically, with the
  user able to inspect what's pending.
- **No new plaintext exposure.** The sync layer transports the
  same `.gpg` ciphertexts that already exist on disk; never
  decrypts.
- **Compose with Phase 5.1.** A user who already pushes to a git
  remote shouldn't have to choose between that and P2P; both work
  side-by-side.

## Non-goals

- **WAN / NAT traversal.** Pairing across the public internet
  needs hole-punching, relays, and trust delegation that this
  milestone is not chasing. LAN only.
- **Real-time replication.** Sub-second propagation isn't needed;
  password-store edits are rare. Eventual consistency in seconds
  is plenty.
- **Federated / many-device topologies (≫2).** The shape we're
  designing for is 2–5 personal devices. Larger fleets work but
  aren't the optimisation target.
- **Replacing git.** Git stays the local history substrate
  (ADR-0004). P2P sync is a transport, not a storage backend.

## Constraints — existing decisions we must respect

- **ADR-0001** — `bypass-core` never speaks OpenPGP. The sync layer
  doesn't need to either; it forwards opaque `.gpg` blobs.
- **ADR-0002** — pass-compatible on-disk layout. After a sync, the
  resulting tree must still be a valid pass store.
- **ADR-0003** — `bypass-core` stays portable. libp2p (or any
  transport) lives in `bypass-cli` (or a new crate), never in core.
- **ADR-0006** — associated `Error` types on trait seams. If we
  introduce a `Sync` trait, it follows the same pattern.
- **ADR-0009** — leak-check before push. Same check must run
  before *any* outbound transmission of blobs, P2P included.

## Threat model

What a hostile party could learn from the sync layer:

- **Passive eavesdropper on the LAN.** Sees encrypted traffic if
  the transport encrypts; otherwise sees `.gpg` ciphertexts (still
  encrypted to the user's keys), metadata (filenames, sizes,
  timing), and the peer identifiers exchanged for pairing.
- **Active attacker on the LAN.** Could MITM unauthenticated
  pairing, masquerading as the user's other device. Once
  authenticated, could replay or reorder messages.
- **Malicious paired peer.** A device the user actually paired
  with that subsequently misbehaves: spam pushes, push
  malformed/oversized blobs to fill the disk, push files that
  don't pass `bypass audit`.
- **Stolen device.** Out of scope (full-device threats sit above
  the sync layer; the store itself is encrypted at rest by
  OpenPGP).

The implications, in order of importance:

1. **Transport must be encrypted and authenticated.** Anything
   less leaks at least metadata (and arguably the structure of the
   store, which is itself sensitive).
2. **Pairing must verify out-of-band.** No "trust on first sight"
   for new peers — the user has to confirm.
3. **The pre-push audit (ADR-0009) applies symmetrically.** A
   misbehaving peer can't make us *receive* something we wouldn't
   push: incoming blobs go through the same allowlist + header
   sniff before being committed locally.
4. **Rate-limit / size-cap incoming traffic.** Even authenticated
   peers shouldn't be able to fill the disk by accident.

## Transport options

### libp2p (`libp2p` crate, Rust)

Mature P2P stack maintained by Parity, used by IPFS, Polkadot, and
many others.

- **Discovery**: `libp2p-mdns` — multicast DNS on the local
  network. No central registry.
- **Transport**: TCP and QUIC. QUIC gives us connection migration
  if the device changes networks (laptop wifi → ethernet).
- **Security**: `libp2p-noise` — Noise XX handshake giving mutual
  authentication and forward-secret encryption. Pairs from an
  identity keypair we generate per device.
- **Application protocols**: `libp2p-request-response` for
  point-to-point requests, `libp2p-gossipsub` for pub-sub
  broadcast (we don't actually need pub-sub at the small fleet
  sizes we're targeting; request-response suffices).
- **Build cost**: ~50 transitive deps, ~30 s incremental build
  added to `bypass-cli`. Heavy but tractable.
- **Portability**: builds on Linux, macOS, Windows, Android
  (verified by IPFS-Lite / Berty), iOS. Wasm support is partial
  but irrelevant for a CLI.
- **Risks**: it's a moving target — minor versions break APIs;
  pin and audit on upgrade. Surface is also broad, so we'd only
  use a narrow slice (mDNS + Noise + request-response).

### Syncthing / magic-wormhole as external dependencies

- **Syncthing** could sync the store directory transparently.
  Already battle-tested, has its own UI for pairing. But pulls in
  a separate daemon (in Go), and the user would have two sync
  layers (Syncthing for the FS, git for history) doing
  overlapping work. Conflicts between Syncthing's rename
  resolution and git's would be confusing.
- **magic-wormhole** is great for one-shot transfers but isn't
  designed for ongoing replication.

Either is fine for users who want to bolt them on, but neither is
a `bypass`-shaped feature.

### Custom protocol (TCP + Noise + bespoke framing)

- **Build cost**: small in dependencies (just `snow` for Noise,
  `tokio` for I/O, mDNS via `mdns-sd`), large in design and
  testing time. Reinvents discovery, peerstore, retry semantics,
  framing.
- **Risk**: every custom protocol has bugs the first six months;
  these would be security-relevant.

### Recommendation: libp2p

The library cost is real but pays for: peer identity, encrypted
authenticated transport, discovery, retry, NAT-friendliness (in
case the LAN ever bridges to a router), and a community of users
who shake out edge cases for us. Custom is plausible only if we
*never* want any of these properties later.

ADR-0010 records this decision.

## Sync semantics options

Three shapes are on the table:

### A. Git-over-libp2p

Use libp2p's request-response protocol to ferry git pack files
between peers. Each peer exposes its repository; pulls drive
`Repository::fetch` from libgit2, pushes drive `git push` to a
custom transport implemented over libp2p.

- **Pros**: reuses Milestone 2's machinery wholesale (history,
  signed commits, rebase). The simplest mental model — "peers are
  remotes".
- **Cons**: every sync potentially conflicts at the git level.
  Encrypted `.gpg` files are opaque to git's merge, so conflicts
  always need human intervention via `bypass edit`. Background
  daemons can't auto-resolve; they'd queue conflicts for the user.
  At the LAN-frequent sync rate we're targeting, conflict UX
  would dominate the experience.

### B. Per-entry replication (last-write-wins)

The sync layer talks `.gpg` blob paths and ciphertexts directly.
Each peer broadcasts (or, more realistically given small fleets:
pushes on-demand) updates. Conflict resolution: timestamp
comparison, last write wins. Git records each accepted update as
a local commit for history, but the wire format isn't git.

- **Pros**: no merge conflicts at sync time; the user always sees
  the latest known version. Better fit for "I edited on my phone,
  the laptop should reflect that within seconds".
- **Cons**: per-entry timestamps are fragile (clock skew, virtual
  machines with reset clocks). Concurrent edits silently lose
  one. The sync layer needs its own metadata channel separate
  from the encrypted blob.

### C. Hybrid: peer-fast-forward with auto-rebase

Wire format is git pack files (option A). Semantics:

1. Devices exchange `git fetch`-style updates over libp2p.
2. If the remote ref is a fast-forward of local HEAD → accept and
   advance HEAD silently.
3. If diverged → run `git rebase --onto <peer>` automatically,
   trusting that conflicting commits on opaque `.gpg` blobs are
   "the peer's view is newer for this entry, just take theirs",
   per a recorded rebase strategy.
4. Conflicts that can't be auto-resolved (same blob touched on
   both sides) surface a `bypass sync status` issue list; the
   user resolves manually via `bypass edit` and the next
   exchange clears them.

- **Pros**: keeps git as the wire format and substrate
  (consistent with ADR-0004 and Milestone 2). Avoids constant
  manual conflict resolution by defaulting to take-theirs when
  histories diverge on a single blob (which is rare on
  password-manager workloads).
- **Cons**: the "take theirs" default is a real *trust* statement;
  if the peer is malicious or buggy, we propagate their version.
  ADR-0009's leak-check on every received pack mitigates the
  worst case (we never accept plaintext, regardless of which
  peer sent it).

### Recommendation: C (hybrid)

Option A is too noisy for the daemon use case. Option B abandons
git as the on-the-wire format, which fragments the system into
two history models. Option C keeps git as both substrate and wire
format, then layers an opinionated rebase policy on top to make
the *common* case (small password store, edits rare) automatic.
Edge cases still drop to the manual conflict UX from Phase 5.1.

ADR-0011 records this decision.

## Pairing UX

The first time two devices want to sync, they need to bootstrap
mutual trust. The options:

1. **Manual fingerprint comparison.** Device A prints its libp2p
   peer ID; user types or QR-scans it on device B; vice versa.
   Each device pins the other's peer ID. This is what SSH does
   for known_hosts.
2. **Pairing PIN (one-shot bootstrap channel).** Device A enters
   pairing mode and shows a 6-digit PIN. User types it on device
   B. Both sides derive a shared secret from the PIN (e.g.
   PAKE / OPAQUE) and use it to authenticate the first Noise
   handshake. After the first connection, peer IDs are pinned and
   the PIN is forgotten.
3. **OpenPGP-key-based.** The shared OpenPGP recipient key (which
   both devices necessarily have if they share the store) signs a
   peer identity. No new pairing flow needed; trust derives from
   "you can decrypt the store, so you're me".

Option 3 is seductively elegant but couples sync trust to the
crypto layer, which would force the sync daemon to talk to gpg
(violating the spirit of "sync layer never sees plaintext", and
adding a pinentry prompt to background sync). Option 1 is the
simplest to implement but ugly to use on a phone. Option 2 is the
right balance: PAKE-from-PIN is well-understood, the PIN can be
displayed once on the device with a screen and entered on the
device adding itself, and after the bootstrap there's no shared
secret to lose.

For the LAN target the practical pairing protocol is:

```
Device A: bypass sync pair --show
   → prints: "PAIRING PIN: 528 491"
Device B: bypass sync pair --enter
   → asks: "PIN from other device:"  user types 528491
   → both devices complete a PAKE-authenticated Noise handshake
   → both pin each other's peer ID
   → PIN is discarded
```

PAKE choice and concrete parameters (PIN format, lifetime,
rate-limit, pinned-peer file layout) are settled in
[ADR-0012](adr/0012-pake-spake2.md): SPAKE2 via the `spake2`
crate; pinned peers live in a single
`$XDG_CONFIG_HOME/bypass/peers.toml` (not a per-peer directory).

## Daemon design

The roadmap mentions "daemon mode + `bypass sync status`". The
sketch:

- New subcommand `bypass sync daemon` (long-running).
- Holds a libp2p `Swarm`, watches the store directory for changes
  via the `notify` crate.
- On local change: commit (via existing `Git2Vcs`), then push to
  paired peers. The existing
  [`Git2Vcs::unfinished_state_name`](../crates/bypass-cli/src/vcs_git2.rs)
  guard applies — the daemon defers auto-commits while the user
  is hand-resolving a conflict.
- On peer push: receive pack, run leak audit on incoming blobs,
  apply per the hybrid policy.
- Exposes a unix socket for `bypass sync status` to query.
- Process-management: launched ad-hoc by the user
  (`bypass sync daemon &`) for v1. Cross-platform
  service-manager glue (systemd user unit on Linux, launchd agent
  on macOS) is scheduled for [Phase 6](ROADMAP.md#phase-6--polish).

Several details that the original draft left implicit have been
pushed into the open-questions section below:

- Socket path on each platform and how to refuse multi-instance
  (open question 9).
- What `bypass sync status` actually displays (open question 10).
- The first-sync bootstrap shape (open question 11).
- The tie-breaker when both peers diverged with disjoint commits
  (open question 12).

Daemon code lives in `bypass-cli` for v1. If it grows enough to
warrant its own crate, the obvious split is `bypass-sync` —
deferred until the code is sized
([ADR-0010](adr/0010-p2p-transport-libp2p.md),
[ADR-0013](adr/0013-sync-transport-trait.md)).

## Conflict resolution at the application layer

Same advice as Phase 5.1's README: when an automatic resolution
can't be made (`bypass sync status` lists an entry as
conflicting), the user runs `bypass edit <entry>` to hand-merge,
then re-runs sync. The hybrid policy makes this rare but it has
to be possible for correctness.

## Summary of recommendations (locked in via ADRs)

| Question | Decision | ADR |
| --- | --- | --- |
| Transport | libp2p (mDNS + Noise + request-response) | [0010](adr/0010-p2p-transport-libp2p.md) |
| Sync semantics | Hybrid: git pack on the wire, auto-rebase-on-divergence, manual fallback | [0011](adr/0011-sync-semantics-hybrid.md) |
| Pairing | PAKE-from-PIN, one-shot bootstrap, then pin peer IDs | covered in ADR-0010 |
| Daemon location | `bypass-cli` for v1; consider `bypass-sync` split later | covered in ADR-0010 |
| Scope | LAN only; 2–5 devices; eventual consistency | this doc |

## Open questions to resolve before implementation

1. **PAKE choice** — **resolved** in
   [ADR-0012](adr/0012-pake-spake2.md): SPAKE2 via the `spake2`
   crate, with 6-digit single-use PIN, 5-minute timeout, and
   daemon-side rate-limiting. The ADR also fixes the
   pinned-peers file format
   (`$XDG_CONFIG_HOME/bypass/peers.toml`).
2. **Daemon supervision** — **resolved**: deferred to
   [Phase 6 (Polish)](ROADMAP.md#phase-6--polish). Sub-milestone
   5.2.c ships the daemon itself runnable in the foreground;
   Phase 6 adds the cross-platform service-management glue
   (systemd user unit on Linux, launchd agent on macOS) with the
   matching `start`/`stop`/`status`/`enable` UX.
3. **`.gitattributes` for `.gpg` files** — **resolved**:
   `bypass init` now writes `.gitattributes` with `*.gpg binary`;
   `bypass sync` lazily installs the rule on stores that pre-date
   it; `bypass doctor` surfaces the missing-rule case. Recorded
   as a Cross-cutting invariant in [`ROADMAP.md`](ROADMAP.md#cross-cutting).
   Phase 5.2.b's merge driver will extend the same line with
   `merge=bypass-take-theirs` per [ADR-0011](adr/0011-sync-semantics-hybrid.md).
4. **Clock handling for any per-entry metadata we might add** —
   **resolved** in [ADR-0014](adr/0014-sync-metadata-and-ordering.md):
   Phase 5.2 adds no per-entry metadata. Git commit fields
   (`author_time`, `committer`, `author_email`) carry everything
   `bypass sync status` needs; each device sets a stable
   `user.name` / `user.email` at daemon startup for attribution.
   Wall-clock time is used only locally (the pairing PIN's
   5-minute timeout, ADR-0012), never for cross-device ordering.
5. **Test strategy** — **resolved** in
   [ADR-0013](adr/0013-sync-transport-trait.md): a request-response
   `Transport` trait with two implementations (`Libp2pTransport`
   for production, `InProcessTransport` for unit tests), both
   living in `bypass-cli` for v1 (not a separate `bypass-sync`
   crate). Three test layers: unit (against the fake), small
   focused libp2p loopback (run by default), and `assert_cmd`
   two-process daemon tests (`#[ignore]` by default, runnable on
   demand).

## Open questions surfaced by the first design pass

These weren't in the original five but emerged on closer review.
None block 5.2.a; each is tagged with the sub-milestone where it
should be resolved and whether it warrants its own ADR.

6. **Identity-key location, perms, and rotation** — **resolved**
   in [ADR-0015](adr/0015-device-identity-key.md): Ed25519
   keypair at `$XDG_CONFIG_HOME/bypass/identity.key` with `0600`
   permissions (refuse-to-load on wider perms), libp2p protobuf
   encoding, atomic write. Rotation via
   `bypass sync identity rotate --confirm` clears `peers.toml`
   (re-pair every device); no automatic rotation.

7. **Peer revocation UX and trust semantics** — **resolved** in
   [ADR-0019](adr/0019-peer-revocation-trust-semantics.md):
   `bypass sync peer rm <name> --yes` removes the pin from
   `peers.toml`; the daemon hot-reloads on the next inbound
   request and refuses subsequent `WantPackFrom` from the revoked
   peer-id. Prior commits authored by the revoked peer are not
   rewritten, consistent with
   [ADR-0014](adr/0014-sync-metadata-and-ordering.md)'s
   no-cryptographic-attribution model.

8. **DoS defences for incoming sync** — **resolved** in
   [ADR-0016](adr/0016-sync-dos-defences.md): 50 MB pack-size
   cap enforced symmetrically (refuse to send and refuse to
   receive); 3-attempt / 60-second per-peer rate-limit window
   mirroring [ADR-0012](adr/0012-pake-spake2.md)'s PAKE
   throttle.

9. **Daemon socket location and multi-instance prevention** —
   **resolved** in [ADR-0017](adr/0017-daemon-socket-location.md):
   `$XDG_RUNTIME_DIR/bypass-sync.sock` on Linux with a documented
   `$TMPDIR/bypass-<uid>-sync.sock` → `/tmp/` fallback chain on
   macOS; `0600` perms; probe-then-bind multi-instance check
   (no pidfile).

10. **`bypass sync status` output shape** — **resolved** in
    [ADR-0018](adr/0018-daemon-status-protocol.md):
    newline-delimited JSON over the ADR-0017 socket; a single
    `status` op returns `{local_peer_id, listening_addrs, peers:
    [{name, peer_id, discovered, last_sync_action,
    last_sync_unix}]}`. `--json` for scripts, fixed-width table
    by default. No "pending conflicts" today — ADR-0011's
    auto-rebase resolves opaque-blob conflicts via the merge
    driver, so there are none to surface; a manual-resolution
    fallback would extend this protocol later.

11. **First-sync (bootstrap) protocol** — **resolved** in
    [ADR-0016](adr/0016-sync-dos-defences.md): the same
    `WantPackFrom { local_head: None, peer_head_seen: None }`
    shape is the bootstrap — no separate message type. The
    responder packs everything reachable from its HEAD; the
    initiator fast-forwards. The same 50 MB pack-size cap
    serves as the per-clone disk-budget guard, with the
    documented escape hatch of bootstrapping via a git remote
    first.

12. **Rebase tie-breaker for symmetric divergence** —
    **resolved** in [ADR-0014](adr/0014-sync-metadata-and-ordering.md):
    peer-ID lexical order. Lower peer-ID rebases onto the higher.
    Clock-free, deterministic, adversary-resistant; both sides
    compute the same answer locally without negotiation.

13. **Battery / background-sync on mobile.** Once Android (Phase
    8) gets the daemon, always-on TCP listeners drain battery.
    Likely needs an interval-poll mode or external scheduler
    integration. Out of scope for Phase 5.2; flagged so Phase 8
    planning doesn't rediscover it.

14. **Cryptographic commit-level attribution (signed commits).**
    [ADR-0014](adr/0014-sync-metadata-and-ordering.md) explicitly
    defers this. Phase 5.2 attributes via `user.name`, which a
    compromised paired peer can spoof. A future ADR — likely
    sometime after Phase 5.2.d ships and we have implementation
    experience — should weigh signing every commit with the
    libp2p identity key plus verification on receive. Until then,
    `bypass sync status` attribution is best-effort, not
    cryptographically authenticated. **Future ADR (post-5.2).**

## Next steps

1. Land this doc + ADR-0010 + ADR-0011.
2. Plan **5.2.a (pairing)**: PAKE crate audit, ADR for PAKE choice,
   implement `bypass sync pair --show` / `--enter`, store paired
   peer IDs and Noise static keys under `~/.config/bypass/peers/`.
3. Plan **5.2.b (sync core)**: in-process two-peer test harness,
   git-pack-over-libp2p, auto-rebase policy.
4. Plan **5.2.c (daemon)**: filesystem watcher, `sync status`
   socket, lifecycle commands.
5. Plan **5.2.d (polish)**: integration tests with a real libp2p
   loopback pair, README rewrite, ROADMAP ticks.

Each sub-milestone gets its own planning session.
