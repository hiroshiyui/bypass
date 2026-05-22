// SPDX-License-Identifier: GPL-3.0-or-later
//
// bypass popup script. Wires the search input + result list to the
// native messaging host described in
// [ADR-0022](../../doc/adr/0022-native-messaging-wire-protocol.md).
//
// Lifecycle: a fresh `NativeClient` opens at DOMContentLoaded; the
// host process spawns at the same moment. Popup close → port close →
// host exits. We never hold long-lived state.
//
// JS-side plaintext lifetime: per [ADR-0023](../../doc/adr/0023-browser-extension-architecture.md),
// JS strings are immutable so we cannot truly zeroize. We drop
// references promptly (no module-level globals hold plaintext) and
// rely on the popup's V8 isolate dying when the popup closes.

import { NativeClient } from "./native.js";
import { unwrapEntries, unwrapPlaintext } from "./types.js";

const SEARCH_DEBOUNCE_MS = 150;

function setStatus(text: string, isError = false): void {
  const el = document.getElementById("status");
  if (!el) return;
  el.textContent = text;
  el.classList.toggle("error", isError);
}

function entryList(): HTMLUListElement {
  const ul = document.getElementById("entries");
  if (!(ul instanceof HTMLUListElement)) {
    throw new Error("popup.html missing #entries");
  }
  return ul;
}

function clearEntries(): void {
  const ul = entryList();
  while (ul.firstChild) ul.removeChild(ul.firstChild);
}

function renderEntries(entries: string[], onPick: (path: string) => void): void {
  clearEntries();
  if (entries.length === 0) {
    setStatus("No matches.");
    return;
  }
  setStatus(`${entries.length} ${entries.length === 1 ? "match" : "matches"}.`);
  const ul = entryList();
  for (const e of entries) {
    const li = document.createElement("li");
    const name = document.createElement("span");
    name.className = "entry-name";
    name.textContent = e;
    li.appendChild(name);

    const copyBtn = document.createElement("button");
    copyBtn.type = "button";
    copyBtn.textContent = "Copy";
    copyBtn.addEventListener("click", (ev) => {
      ev.stopPropagation();
      onPick(e);
    });
    li.appendChild(copyBtn);
    ul.appendChild(li);
  }
}

async function copyFirstLineToClipboard(
  client: NativeClient,
  path: string,
): Promise<void> {
  setStatus(`Decrypting ${path}…`);
  let plaintext: string | null = null;
  try {
    const reply = await client.request({ op: "show", path });
    plaintext = unwrapPlaintext(reply);
    // Pass convention: the first line is the password; subsequent
    // lines are `key: value` metadata. The clipboard gets just the
    // password.
    const newlineIdx = plaintext.indexOf("\n");
    const first = newlineIdx < 0 ? plaintext : plaintext.slice(0, newlineIdx);
    await navigator.clipboard.writeText(first);
    setStatus(`Copied ${path}. Clipboard will need to be cleared manually.`);
  } catch (e) {
    setStatus(
      `Copy failed: ${e instanceof Error ? e.message : String(e)}`,
      true,
    );
  } finally {
    // Drop the plaintext reference; V8 will eventually GC it. The
    // popup itself will be destroyed when the user clicks away.
    plaintext = null;
    void plaintext;
  }
}

async function loadAll(client: NativeClient): Promise<string[]> {
  setStatus("Loading…");
  const reply = await client.request({ op: "ls" });
  return unwrapEntries(reply);
}

async function search(client: NativeClient, pattern: string): Promise<string[]> {
  setStatus("Searching…");
  const reply = await client.request({ op: "find", pattern });
  return unwrapEntries(reply);
}

function debounce<T extends (...args: never[]) => void>(
  fn: T,
  ms: number,
): (...args: Parameters<T>) => void {
  let h: ReturnType<typeof setTimeout> | null = null;
  return (...args: Parameters<T>) => {
    if (h !== null) clearTimeout(h);
    h = setTimeout(() => fn(...args), ms);
  };
}

async function main(): Promise<void> {
  let client: NativeClient;
  try {
    client = new NativeClient();
  } catch (e) {
    setStatus(
      `Cannot reach bypass host: ${e instanceof Error ? e.message : String(e)}. ` +
        `Did you run \`bypass messaging-host install\`?`,
      true,
    );
    return;
  }

  const onPick = (path: string) => {
    void copyFirstLineToClipboard(client, path);
  };

  // Initial load: show everything.
  try {
    const entries = await loadAll(client);
    renderEntries(entries, onPick);
  } catch (e) {
    setStatus(
      `Initial load failed: ${e instanceof Error ? e.message : String(e)}`,
      true,
    );
    return;
  }

  const q = document.getElementById("q");
  if (!(q instanceof HTMLInputElement)) {
    throw new Error("popup.html missing #q");
  }

  const runSearch = debounce(async (pattern: string) => {
    try {
      const entries =
        pattern.length === 0
          ? await loadAll(client)
          : await search(client, pattern);
      renderEntries(entries, onPick);
    } catch (e) {
      setStatus(
        `Search failed: ${e instanceof Error ? e.message : String(e)}`,
        true,
      );
    }
  }, SEARCH_DEBOUNCE_MS);

  q.addEventListener("input", () => runSearch(q.value.trim()));

  // Tear the host down when the popup unloads, even though the
  // browser does this for us — keeps the intent visible.
  window.addEventListener("unload", () => client.close());
}

document.addEventListener("DOMContentLoaded", () => {
  void main();
});
