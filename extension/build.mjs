// SPDX-License-Identifier: GPL-3.0-or-later
//
// Build script for the bypass WebExtension. Bundles `src/popup.ts`
// (which transitively pulls in `native.ts` + `types.ts`) into
// `dist/popup.js`, copies `popup.html` and `manifest.json` into
// `dist/`, and (optionally) emits a `bypass-extension-<v>.zip` ready
// for AMO / Chrome Web Store submission.
//
// Run from `extension/`:
//
//     npm ci
//     node build.mjs           # writes dist/
//     node build.mjs --zip     # also writes the zip alongside dist/

import { build } from "esbuild";
import { copyFile, mkdir, readFile, rm, stat, readdir, writeFile } from "node:fs/promises";
import { createReadStream } from "node:fs";
import { spawn } from "node:child_process";
import path from "node:path";

const DIST = "dist";
const SRC = "src";

async function exists(p) {
  try {
    await stat(p);
    return true;
  } catch {
    return false;
  }
}

async function main() {
  const args = new Set(process.argv.slice(2));

  // Clean slate so a stale file in dist/ can't sneak into a release.
  if (await exists(DIST)) {
    await rm(DIST, { recursive: true, force: true });
  }
  await mkdir(DIST, { recursive: true });

  // Bundle the popup. esbuild walks the import graph from
  // `src/popup.ts` and pulls in `src/native.ts` + `src/types.ts`.
  // `iife` format keeps the result loadable directly from a `<script>`
  // tag in `popup.html` — no module loader, no chunking.
  await build({
    entryPoints: [path.join(SRC, "popup.ts")],
    bundle: true,
    format: "iife",
    target: "es2022",
    outfile: path.join(DIST, "popup.js"),
    sourcemap: false,
    minify: false,
    legalComments: "inline",
  });

  // Static assets.
  await copyFile(path.join(SRC, "popup.html"), path.join(DIST, "popup.html"));
  await copyFile("manifest.json", path.join(DIST, "manifest.json"));

  console.log(`Built to ${DIST}/`);

  if (args.has("--zip")) {
    const manifest = JSON.parse(await readFile("manifest.json", "utf8"));
    const zipName = `bypass-extension-${manifest.version}.zip`;
    await zipDir(DIST, zipName);
    console.log(`Zipped to ${zipName}`);
  }
}

// Use the system `zip` tool because Node has no built-in
// zip-writer. AMO and Chrome Web Store both accept a plain
// `zip` archive of the dist/ contents (no top-level dir).
async function zipDir(dir, outZip) {
  const entries = await readdir(dir);
  // Remove an existing zip first so `-r` doesn't append.
  if (await exists(outZip)) {
    await rm(outZip);
  }
  await new Promise((resolve, reject) => {
    const child = spawn("zip", ["-r", path.resolve(outZip), ...entries], {
      cwd: path.resolve(dir),
      stdio: ["ignore", "pipe", "inherit"],
    });
    child.on("error", reject);
    child.on("exit", (code) => {
      if (code === 0) resolve(undefined);
      else reject(new Error(`zip exited with code ${code}`));
    });
  });
}

// Quiet a lint warning about unused imports under tools that don't
// realise the build script uses these dynamically.
void createReadStream;
void writeFile;

await main();
