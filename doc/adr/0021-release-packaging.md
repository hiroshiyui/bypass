<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Release packaging: hand-rolled GitHub Actions workflow for v0.1.x

* Status: accepted
* Date: 2026-05-22
* Deciders: hiroshiyui

## Context and Problem Statement

Phase 6's last open item is "release packaging (cargo-dist or
similar)". With Phase 1–6 features now shipping CI-green on
Linux and macOS, we need a way to cut a tagged release that
produces downloadable binaries for the users who don't want to
build from source.

Two questions:

1. **Which build tool drives it** — `cargo-dist`, a hand-rolled
   GitHub Actions workflow, or something else?
2. **Which artefacts do we ship in v0.1.x** — just binary
   tarballs? Homebrew taps? .deb / .rpm packages?

The answer needs to match `bypass`'s pre-audit, pre-1.0
status: predictable, easy to reason about, easy to throw away
if Phase 7 (browser extension) changes what "a release"
includes.

## Considered Options

**Build tool:**

* **Hand-rolled GitHub Actions workflow.** ~70 lines of YAML in
  `.github/workflows/release.yml`. We own every step. Direct
  control over toolchain pinning, cross-compilation, artifact
  naming, attestation. No external tool to track.
* `cargo-dist` (renamed to `dist` in late 2024). Generates the
  workflow + an installer.sh / installer.ps1 + a Homebrew tap.
  Active project, but multiple breaking-change cycles in the
  last 18 months; a small project pinning to a specific version
  pays the maintenance cost without seeing the convenience
  payoff until there are real users.
* `goreleaser` / `cross-rs` directly / per-distro CI. None
  obviously better for our shape (single binary, two OSes, no
  Windows).

**Cross-compilation:**

* **Per-target runner.** Build `x86_64-unknown-linux-gnu` on
  `ubuntu-latest`, `aarch64-unknown-linux-gnu` on
  `ubuntu-latest` via `cross` (containerised toolchain),
  `x86_64-apple-darwin` and `aarch64-apple-darwin` on
  `macos-latest` (the Apple Silicon runner cross-compiles to
  x86_64 natively). Four artefacts per tag.
* All-in-one runner with multi-arch via `cross`. Slower; no
  meaningful saving over the parallel matrix.

**Artefact shape:**

* **Compressed tarballs** named
  `bypass-vX.Y.Z-<target>.tar.gz`, each containing the
  `bypass` binary, `LICENSE`, `README.md`, plus a top-level
  `SHA256SUMS` file in the GitHub Release alongside the tarballs.
  Standard format every Unix user can `tar xzf`. No installer
  script in v0.1.x — users `cp bypass ~/.local/bin/` or wire it
  into a Nix flake themselves.
* Native packages (.deb, .rpm). One per distro per arch
  multiplies the matrix. Reserved for after a real user asks.
* Homebrew tap. Tempting on macOS but adds an external repo to
  maintain. Defer.

**Trigger:**

* **Git tag matching `v*`.** `v0.1.0`, `v0.1.1`, etc.
  `softprops/action-gh-release` auto-creates the Release.
* Manual workflow_dispatch. Useful for dry-runs; can be added
  later without breaking the tag-trigger contract.

## Decision Outcome

- **Build tool:** hand-rolled GitHub Actions workflow at
  `.github/workflows/release.yml`. Roughly 70–80 lines of YAML;
  matches the shape of the existing `ci.yml`. **No `cargo-dist`
  for v0.1.x.** Re-evaluate after the first hand-rolled release
  surfaces what we wish we had.

- **Targets (v0.1.x):**
  - `x86_64-unknown-linux-gnu` (built on `ubuntu-latest`)
  - `aarch64-unknown-linux-gnu` (built on `ubuntu-latest` via `cross`)
  - `x86_64-apple-darwin` (built on `macos-latest`)
  - `aarch64-apple-darwin` (built on `macos-latest`)

  No Windows: the daemon is Unix-only (ADR-0017); release
  artefacts mirror what we actually support.

- **Artefacts per tag:**
  - One `bypass-vX.Y.Z-<target>.tar.gz` per target, containing
    the `bypass` binary + `LICENSE` + `README.md`.
  - One `SHA256SUMS` file at the Release root.

- **Trigger:** `on: push: tags: ['v*']`. Pushing a tag matching
  the pattern fires the workflow; the existing `ci.yml`
  workflow continues to run on every push to `main` and every
  PR, independently.

- **Versioning:** workspace version stays in `Cargo.toml`
  (`version = "0.1.0"`); release tags exactly mirror it
  (`git tag v0.1.0`). The workflow extracts the version from
  the tag at run time via `${{ github.ref_name }}`, so no
  separate version file to maintain.

- **Out of scope for v0.1.x:**
  - `cargo-dist` / `dist`
  - Homebrew tap
  - .deb / .rpm / AUR packages
  - Windows binaries
  - signed releases / SLSA attestation (Phase-7-or-later)

## Consequences

### Good

- One YAML file is the entire release pipeline. A new contributor
  can read it top-to-bottom in two minutes.
- No external version-bump tool to keep in sync with `Cargo.toml`.
- Re-evaluable: if `cargo-dist` looks compelling after our first
  real tag, switching is a YAML rewrite, not a code change.
- Matches CI's `dtolnay/rust-toolchain` + `Swatinem/rust-cache`
  pattern, so toolchain drift between CI and release is hard.

### Bad

- No installer scripts. macOS users on Apple Silicon who don't
  use `rustup` get a tarball, not a one-liner. Acceptable for
  pre-1.0; a real user complaint is the trigger to revisit.
- No Homebrew tap. Same trade-off — pre-1.0, defer until a
  real user asks.
- We own keeping the matrix in sync with what `cross` actually
  supports. `cross` updates infrequently and breaks rarely;
  this is small surface.
- aarch64-linux-gnu via `cross` requires Docker on the runner
  — `ubuntu-latest` has it pre-installed, but if a future
  runner image drops Docker we'll notice at the next release
  tag.

## Confirmation

- `.github/workflows/release.yml` exists and is the workflow
  referenced here. The next `v*` tag pushed to the repo triggers
  it; until then, the YAML is unverified beyond syntax
  parsing.
- README's "Releases" line points at
  `https://github.com/hiroshiyui/bypass/releases`. That URL is
  empty until the first tag is cut.
- ROADMAP Phase 6 release-packaging checkbox ticks.

## Related ADRs

- [ADR-0005](0005-gpl-license-with-spdx-headers.md): release
  tarballs include `LICENSE`, the same way every source file
  carries an SPDX header.
- [ADR-0017](0017-daemon-socket-location.md): the daemon being
  Unix-only is the reason the release matrix is Unix-only.
- [ADR-0020](0020-daemon-service-supervision.md): the service-
  supervision install paths assume the binary on disk knows
  its own absolute path (`current_exe()`); a release tarball
  satisfies that once the user `cp`s the binary to its final
  location and re-runs `bypass sync daemon install`.
