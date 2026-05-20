---
name: release-engineering
description: Manage the full software release process, including version bumps, changelogs, Git tags, and GitHub releases.
---

When performing release engineering for `bypass`, always follow these steps:

1. **Verify the build is clean** — run the full check suite from a clean state:
   ```sh
   cargo clean
   cargo build --release
   cargo test
   cargo clippy --all-targets -- -D warnings
   cargo fmt --check
   ```
   All must pass before proceeding. A release that ships with warnings or formatting drift is a process failure.

2. **Run the security audit** — apply the `security-audit` skill. A password manager release must not ship with any **Critical** or **High** finding open. `cargo audit` must report zero unpatched advisories in the dependency tree.

3. **Determine the release type** — review all unreleased commits since the last tag and classify the release as `major`, `minor`, or `patch` following [Semantic Versioning](https://semver.org/). Until `1.0.0`, breaking changes may land in minor bumps, but they must still be called out in the changelog. Present the recommendation to the user and confirm before proceeding.

4. **Update the version** — bump the `version` field in `Cargo.toml` to match the new release version. Run `cargo build` once to refresh `Cargo.lock`.

5. **Update `CHANGELOG.md`** — add a new version entry at the top following the [Keep a Changelog](https://keepachangelog.com/) format. Group changes under `Added`, `Changed`, `Fixed`, `Removed`, or `Security` as appropriate. Highlight any change that affects on-disk store layout or pass-compatibility — users may need migration steps.

6. **Reconcile `doc/ROADMAP.md`** — confirm that every milestone whose work is included in this release has its checkboxes ticked. If a phase is fully complete, note it (e.g., add `*(complete in vX.Y.Z)*` next to the phase heading).

7. **Commit the release** — stage `Cargo.toml`, `Cargo.lock`, `CHANGELOG.md`, and `doc/ROADMAP.md` together and commit with the message `chore: release vX.Y.Z`.

8. **Tag the release** — create an annotated Git tag (`git tag -a vX.Y.Z -m "vX.Y.Z"`) and push both the commit and the tag to the remote (`git push && git push --tags`).

9. **Create a GitHub release** — use `gh release create vX.Y.Z --title "vX.Y.Z" --notes "..."` with the corresponding `CHANGELOG.md` section as the release notes. Note: use `--notes` (not `--body`) for the release description. Attach prebuilt binaries if release-time packaging is configured.
