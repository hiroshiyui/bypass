// SPDX-License-Identifier: GPL-3.0-or-later
//
// Promise-based client for the native-messaging host described in
// [ADR-0022](../../doc/adr/0022-native-messaging-wire-protocol.md).
//
// Lifecycle: one `NativeClient` opens a `chrome.runtime.connectNative`
// port the moment it's constructed. The popup creates one client per
// invocation; closing the popup tears the port (and the host process)
// down naturally. We never hold a long-lived connection; v1 has no
// background worker.
//
// Request correlation: each `request()` call gets a unique numeric
// `id`. The port's `onMessage` listener routes the reply back to the
// pending promise. If the port closes before a pending request gets
// a reply, every pending promise rejects with a port-closed error.

import type { Reply, RequestBody } from "./types.js";

const HOST_NAME = "io.bypass.host";

/** Opens a fresh native-messaging port at construction time. */
export class NativeClient {
  private port: chrome.runtime.Port;
  private nextId = 1;
  private pending = new Map<
    number,
    { resolve: (r: Reply) => void; reject: (e: Error) => void }
  >();
  private closed = false;
  private closeError: Error | null = null;

  constructor() {
    this.port = chrome.runtime.connectNative(HOST_NAME);
    this.port.onMessage.addListener((msg: unknown) => this.onMessage(msg));
    this.port.onDisconnect.addListener(() => this.onDisconnect());
  }

  /** Send `body` and resolve with the matching reply. The reply may
   * itself be an error reply (`ok: false`) — callers should use the
   * `unwrap*` helpers in `types.ts` to surface that as a thrown
   * Error. */
  request(body: RequestBody): Promise<Reply> {
    if (this.closed) {
      return Promise.reject(
        this.closeError ?? new Error("native port already closed"),
      );
    }
    const id = this.nextId++;
    return new Promise<Reply>((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      try {
        this.port.postMessage({ id, ...body });
      } catch (e) {
        this.pending.delete(id);
        reject(e instanceof Error ? e : new Error(String(e)));
      }
    });
  }

  /** Close the port and fail any still-pending requests. Idempotent. */
  close(): void {
    if (this.closed) return;
    this.closed = true;
    try {
      this.port.disconnect();
    } catch {
      // Already disconnected — ignore.
    }
    const err = this.closeError ?? new Error("native port closed by client");
    for (const { reject } of this.pending.values()) {
      reject(err);
    }
    this.pending.clear();
  }

  private onMessage(msg: unknown): void {
    // The host can only emit `Reply` shapes; runtime-validate
    // defensively to surface a clear error instead of a silent
    // type-cast.
    if (
      typeof msg !== "object" ||
      msg === null ||
      typeof (msg as { id?: unknown }).id !== "number" ||
      typeof (msg as { ok?: unknown }).ok !== "boolean"
    ) {
      console.error("bypass: malformed reply from host:", msg);
      return;
    }
    const reply = msg as Reply;
    const slot = this.pending.get(reply.id);
    if (!slot) {
      console.warn("bypass: reply with unknown id:", reply.id);
      return;
    }
    this.pending.delete(reply.id);
    slot.resolve(reply);
  }

  private onDisconnect(): void {
    const last = chrome.runtime.lastError;
    this.closeError = new Error(
      `native host disconnected${last?.message ? `: ${last.message}` : ""}`,
    );
    this.closed = true;
    for (const { reject } of this.pending.values()) {
      reject(this.closeError);
    }
    this.pending.clear();
  }
}
