<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Browser extension architecture: Manifest V3, single TypeScript codebase

* Status: accepted
* Date: 2026-05-22
* Deciders: hiroshiyui

## Context and Problem Statement

[ADR-0022](0022-native-messaging-wire-protocol.md) settled the
wire format between the desktop `bypass` binary and the
browser-side extension. This ADR settles the *extension* side:
which manifest version, which build pipeline, which UI shape,
and what the security boundary looks like.

Constraints:

- One codebase has to load in both **Firefox** and **Chrome**
  (the Phase 7 ROADMAP scope; Brave / Edge / Vivaldi /
  Chromium inherit Chrome's machinery and load the same
  artefact unchanged).
- Phase 7.2 ships a **popup MVP only**: search the store +
  reveal-and-copy-to-clipboard. Autofill is deferred to 7.2.b.
- The extension *never* sees a GPG key, an OpenPGP packet, or
  the store directory. Every privileged op goes back over the
  native-messaging pipe to the host. ADR-0022's seven-op
  surface is the whole API.
- No npm runtime deps (only build-time). Supply-chain surface
  for a password manager has to stay small.

## Considered Options

**Manifest version:**

* **Manifest V3.** Chrome MV2 is EOL (June 2024); Firefox
  added MV3 in 109 and treats it as the forward path.
  Same code loads in both with a single
  `browser_specific_settings.gecko.id` field for Firefox.
* MV2. Still works in Firefox but is permanently end-of-life
  in Chrome and would force a fork the moment we ship to CWS.

**Codebase layout:**

* **Single TypeScript codebase under `extension/`.** Build
  produces one MV3 artefact that loads in both browsers.
  `esbuild` bundles + `tsc` type-checks (`--noEmit`).
* Separate Firefox / Chrome builds. Doubles the maintenance
  surface for what is functionally identical code.
* Pure JS, no TS. Less tooling, less safety. The wire-format
  types from ADR-0022 are exactly the place where a typed
  interface earns its keep.

**UI framework:**

* **Vanilla DOM.** The popup MVP is a `<input>` and a `<ul>`
  with a few hundred lines of TS at most. Pulling in
  lit-html / preact / svelte would be more setup than UI.
* lit-html / preact. Worth revisiting if the popup ever grows
  past ~200 lines or autofill adds a content-script with its
  own DOM injection. Defer.

**Build / bundle tooling:**

* **`esbuild` for bundling + `typescript` for type-check.**
  Both are devDeps; bundles to a single `dist/` directory the
  browsers load. `esbuild` is fast, has no runtime, and is
  itself maintained by the Rust-extension ecosystem we already
  trust (the same shop ships `esbuild-rs`).
* `webpack` / `rollup` / `vite`. All more complex; none
  produce a meaningfully smaller bundle for our shape.

**Background worker:**

* **None for v1.** The popup opens a native-messaging port
  directly via `chrome.runtime.connectNative`. The port lives
  for the popup's lifetime; closing the popup tears the port
  (and the host process) down. Adequate for search + copy.
* Persistent service worker. Required when autofill (7.2.b)
  adds content scripts that need long-lived host state.
  Adding it now would be premature.

**Security boundary:**

* **No `externally_connectable`** in `manifest.json` → web
  pages cannot call `chrome.runtime.sendMessage` against our
  extension; only the popup script can reach the native port.
* The native host's manifest pins the extension id (Firefox
  `allowed_extensions`, Chrome `allowed_origins`) so a
  rogue extension on the same machine can't impersonate us
  and spawn the host.
* JavaScript strings are immutable; we **cannot** truly
  zeroize plaintext in the popup's V8 heap. We drop
  references promptly and document the limitation.

**Store submission:**

* **Document the manual flow; don't automate.** `build.mjs`
  emits a loadable zip and a README points at AMO + CWS dev
  consoles. Submission requires publisher accounts and policy
  review that no CI can do.
* Automate via `web-ext` + the CWS API. Real value only after
  the first successful manual submission shapes the workflow;
  defer.

## Decision Outcome

- **Manifest V3.** Same artefact loads in Firefox + Chrome.
  Firefox-specific bits go in
  `browser_specific_settings.gecko.id`; everything else is
  cross-browser.
- **Layout** under `extension/`:
  ```
  extension/
  ├── manifest.json
  ├── src/
  │   ├── popup.html
  │   ├── popup.ts          # search input + entry list + copy
  │   ├── native.ts         # typed wrapper around connectNative
  │   └── types.ts          # mirrors ADR-0022 wire schema
  ├── icons/                # 16/48/128 PNGs
  ├── package.json          # typescript + esbuild (devDeps)
  ├── tsconfig.json         # strict, ES2022, DOM lib
  ├── build.mjs             # esbuild bundle + zip
  └── README.md             # install (load unpacked) + troubleshoot
  ```
- **UI**: vanilla DOM for v1. No framework. Revisit at the
  earliest of (a) popup > 200 LoC, (b) autofill ships, or
  (c) a user reports a UX issue that needs declarative
  rendering.
- **Build**: `npm ci` then `node build.mjs`. The script runs
  `esbuild` on the two entry points
  (`src/popup.ts`, `src/native.ts`, the latter shared by the
  popup) targeting ES2022, copies `manifest.json` + icons to
  `dist/`, and zips `dist/` to
  `extension/bypass-extension-<version>.zip` for store
  submission.
- **Background worker**: not present in v1. `manifest.json`
  declares no `background` field. Autofill (7.2.b) adds one.
- **Security boundary**:
  - `manifest.json` has no `externally_connectable`.
  - The native-host manifest written by
    [`bypass messaging-host install`](../../crates/bypass-cli/src/native_host_install.rs)
    pins extension ids via `allowed_extensions` (Firefox) /
    `allowed_origins` (Chrome).
  - JS-side plaintext lifetime: ephemeral. The popup's V8
    isolate is destroyed when the popup closes; we document
    that JS strings cannot be cryptographically scrubbed
    while held.
- **Store submission**: deferred. `build.mjs` produces a
  loadable / submittable zip; the README explains
  `load unpacked` (development) and `submit to AMO/CWS`
  (production). No CI automation.

## Consequences

### Good

- One MV3 artefact runs in two browsers without per-browser
  forks; Chrome-family browsers (Brave, Edge, Vivaldi,
  Chromium) inherit the install path and work for free.
- The TypeScript wire-format types in `src/types.ts` mirror
  ADR-0022 line-by-line; a future schema mismatch is a
  compile error, not a runtime mystery.
- Build pipeline is two dev-deps and one 50-line build script.
  CI's new `extension-typecheck` job (introduced alongside this
  ADR) keeps the TS surface compiling on every push.
- Security boundary is minimal-by-construction: web pages
  can't talk to the extension, and a rogue local extension
  can't impersonate us to the host. JS-heap plaintext lifetime
  is documented honestly (a limitation, not a "fix").

### Bad

- No autofill in v1. Users get search + copy, not in-page
  injection. Real autofill needs content scripts + the
  background worker + careful click-to-fill UX; that's 7.2.b.
- Plaintext lives in V8 heap during display. We can't
  truly zeroize JS strings; we can only drop references and
  hope V8 GCs the page promptly. This is the same trade-off
  every browser-based password manager makes; documented in
  the popup's source comments.
- Vanilla DOM means hand-written event wiring. Acceptable for
  the popup MVP; revisit if the surface grows.
- The 512 KB reply cap (ADR-0022) means extremely large
  entries (e.g. a stored SSH key + a long body) can't render
  in the popup. Falls back to the CLI with a clear error;
  documented in the README.

## Confirmation

- The extension exists at `extension/` in the repo. `build.mjs`
  emits a `dist/` and a zip; the README walks through
  `load unpacked` for both browsers.
- CI's `extension-typecheck` job (added in the same Phase 7
  commit series) runs `tsc --noEmit` + `node build.mjs` on
  every push. A type-error or bundle-failure fails CI.
- The native host's manifest pins the extension id; verified
  by the `messaging_host_install_*` integration tests under
  `crates/bypass-cli/tests/end_to_end.rs`.

## Related ADRs

- [ADR-0022](0022-native-messaging-wire-protocol.md): defines
  the protocol this extension consumes.
- [ADR-0001](0001-platform-delegated-crypto.md): the
  extension is a thin UI; all crypto stays in the desktop
  binary's `gpg` subprocess path.
- [ADR-0017](0017-daemon-socket-location.md) +
  [ADR-0018](0018-daemon-status-protocol.md): the *other*
  local-IPC channel `bypass` uses. The two channels are
  deliberately not unified — sync-daemon runs on a Unix
  socket talking JSON; the native host runs on a stdin/stdout
  pipe talking length-prefixed JSON. Different consumers,
  different OS-level mechanisms.
