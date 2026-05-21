// SPDX-License-Identifier: GPL-3.0-or-later

//! Cryptographically-secure password generation.
//!
//! Uses `rand::rng()`, which in `rand` 0.9 returns a thread-local
//! ChaCha-based CSPRNG seeded from the OS entropy pool — suitable for
//! generating secrets. Selection from the alphabet is via
//! `Rng::random_range`, which does rejection sampling internally and so
//! is free of modulo bias.
//!
//! Default length is 25 characters, matching `pass`'s `--length` default.

use rand::Rng;

/// Letters and digits (62 chars). Excludes look-alikes? No — at this
/// length collisions don't matter and excluding them weakens entropy by
/// a few bits per character. Keep the full alphabet.
pub const ALPHANUMERIC: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

/// Common punctuation safe across shells and most URL/form fields. We
/// deliberately omit space, quote characters (`'`, `"`), and backtick to
/// reduce friction when pasting passwords into command-line tools.
pub const SYMBOLS: &[u8] = b"!@#$%^&*()-_=+[]{}|;:,.<>?/~";

pub const DEFAULT_LENGTH: usize = 25;

/// Generate a password of `length` characters drawn uniformly from
/// [`ALPHANUMERIC`] plus, optionally, [`SYMBOLS`].
pub fn generate(length: usize, with_symbols: bool) -> String {
    if length == 0 {
        return String::new();
    }
    let mut alphabet: Vec<u8> = ALPHANUMERIC.to_vec();
    if with_symbols {
        alphabet.extend_from_slice(SYMBOLS);
    }
    let mut rng = rand::rng();
    let mut out = String::with_capacity(length);
    for _ in 0..length {
        let i = rng.random_range(0..alphabet.len());
        out.push(alphabet[i] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_length_yields_empty_string() {
        assert_eq!(generate(0, true), "");
        assert_eq!(generate(0, false), "");
    }

    #[test]
    fn default_length_constant_matches_pass() {
        assert_eq!(DEFAULT_LENGTH, 25);
    }

    #[test]
    fn produced_password_has_requested_length() {
        for len in [1, 8, 25, 64, 256] {
            assert_eq!(generate(len, true).chars().count(), len);
            assert_eq!(generate(len, false).chars().count(), len);
        }
    }

    #[test]
    fn no_symbols_mode_contains_only_alphanumerics() {
        // One long sample is enough: at length 1024 the chance any
        // non-alphanumeric leaks in is zero for a correct implementation.
        let pw = generate(1024, false);
        for c in pw.chars() {
            assert!(
                c.is_ascii_alphanumeric(),
                "non-alphanumeric `{c}` in no-symbols password"
            );
        }
    }

    #[test]
    fn with_symbols_mode_actually_includes_symbols() {
        // P(no symbol in 200 chars) = (62/90)^200 ≈ 10^-32, so any failure
        // here means symbol generation is broken, not flakiness.
        let pw = generate(200, true);
        assert!(
            pw.chars().any(|c| SYMBOLS.contains(&(c as u8))),
            "with_symbols=true produced no symbols: {pw}"
        );
    }

    #[test]
    fn every_character_is_from_the_advertised_alphabet() {
        let mut alphabet: Vec<u8> = ALPHANUMERIC.to_vec();
        alphabet.extend_from_slice(SYMBOLS);
        let pw = generate(1024, true);
        for c in pw.chars() {
            assert!(
                alphabet.contains(&(c as u8)),
                "char `{c}` (0x{:02x}) is not in the advertised alphabet",
                c as u8
            );
        }
    }

    #[test]
    fn two_invocations_almost_certainly_differ() {
        // Two 32-char passwords from a CSPRNG collide with probability
        // ~62^-32 (or 90^-32 with symbols). If this ever fires, the RNG
        // is broken — not just unlucky.
        assert_ne!(generate(32, false), generate(32, false));
        assert_ne!(generate(32, true), generate(32, true));
    }
}
