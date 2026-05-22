// SPDX-License-Identifier: GPL-3.0-or-later
//
// TypeScript mirror of the ADR-0022 wire schema. Keep in lockstep
// with `crates/bypass-cli/src/messaging_host.rs`; the two ends are
// canonical to each other and the wire is the contract.

/** Request envelope sent from the popup to the native host. */
export type Request = { id: number } & RequestBody;

/** Op-specific payloads. The `op` field is the discriminator. */
export type RequestBody =
  | { op: "ls"; subpath?: string }
  | { op: "find"; pattern: string }
  | { op: "show"; path: string; field?: string }
  | {
      op: "insert";
      path: string;
      plaintext: string;
      overwrite?: boolean;
    }
  | {
      op: "generate";
      path: string;
      length?: number;
      symbols?: boolean;
      in_place?: boolean;
      force?: boolean;
    }
  | { op: "otp"; path: string }
  | { op: "rm"; path: string; recursive?: boolean };

/** Reply envelope. Discriminated by the `ok` flag. */
export type Reply = OkReply | ErrReply;

export type OkReply = { id: number; ok: true } & OkBody;

export type OkBody =
  | { entries: string[] }
  | { plaintext: string }
  | { value: string }
  | { password: string }
  | { code: string }
  | Record<string, never>;

export type ErrReply = { id: number; ok: false; error: string };

/** Narrow the reply union by op. Throws if the host returned an
 * error reply or an unexpected shape. */
export function unwrapEntries(r: Reply): string[] {
  if (!r.ok) throw new Error(r.error);
  if (!("entries" in r)) throw new Error("reply missing `entries`");
  return r.entries;
}

export function unwrapPlaintext(r: Reply): string {
  if (!r.ok) throw new Error(r.error);
  if (!("plaintext" in r)) throw new Error("reply missing `plaintext`");
  return r.plaintext;
}

export function unwrapValue(r: Reply): string {
  if (!r.ok) throw new Error(r.error);
  if (!("value" in r)) throw new Error("reply missing `value`");
  return r.value;
}

export function unwrapPassword(r: Reply): string {
  if (!r.ok) throw new Error(r.error);
  if (!("password" in r)) throw new Error("reply missing `password`");
  return r.password;
}

export function unwrapCode(r: Reply): string {
  if (!r.ok) throw new Error(r.error);
  if (!("code" in r)) throw new Error("reply missing `code`");
  return r.code;
}

export function unwrapEmpty(r: Reply): void {
  if (!r.ok) throw new Error(r.error);
}
