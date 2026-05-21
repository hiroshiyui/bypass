// SPDX-License-Identifier: GPL-3.0-or-later

//! Multi-line entry parsing: first line is the password, remaining lines
//! are `key: value` fields (compatible with the `pass` convention).
//!
//! Example entry body:
//!
//! ```text
//! hunter2
//! login: alice
//! url: https://example.com
//! otpauth://totp/Example:alice?secret=JBSWY3DPEHPK3PXP&issuer=Example
//! note: a stray line without a colon is preserved but not addressable as a field
//! ```
//!
//! The password is `hunter2`. `entry.field("login")` returns
//! `Some("alice")`. `entry.field("URL")` returns `Some("https://...")`
//! — field lookup is case-insensitive (`pass-otp` and other extensions
//! rely on this).
//!
//! Lines that don't look like `key:` followed by a value are preserved
//! in the entry but are not reachable through [`Entry::field`]. The
//! `otpauth://…` line in the example above happens to have a `:` after
//! the scheme, so it *is* indexed under `otpauth` — that's how `pass-otp`
//! finds it.

#[derive(Debug, thiserror::Error)]
pub enum EntryError {
    #[error("entry contents are not valid UTF-8")]
    NotUtf8,
}

/// A parsed pass-style entry.
#[derive(Debug, Clone)]
pub struct Entry {
    password: String,
    /// Fields in their original order. Keys are stored lowercased so
    /// lookup is case-insensitive; the *value* preserves the original
    /// casing/spelling exactly as the user wrote it.
    fields: Vec<(String, String)>,
}

impl Entry {
    /// Parse `bytes` as a pass-style entry. Requires valid UTF-8.
    pub fn parse(bytes: &[u8]) -> Result<Self, EntryError> {
        let text = std::str::from_utf8(bytes).map_err(|_| EntryError::NotUtf8)?;
        Ok(Self::parse_str(text))
    }

    /// Parse from an already-validated `&str`. Useful when the caller
    /// has already converted from `SecretBytes`.
    pub fn parse_str(text: &str) -> Self {
        // pass tolerates trailing CR (CRLF stores) — strip it from every
        // line, including the password.
        let mut lines = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l));
        let password = lines.next().unwrap_or("").to_owned();
        let mut fields = Vec::new();
        for line in lines {
            if line.is_empty() {
                continue;
            }
            if let Some((k, v)) = line.split_once(':') {
                let key = k.trim();
                if key.is_empty() {
                    continue;
                }
                let value = v.trim();
                fields.push((key.to_lowercase(), value.to_owned()));
            }
        }
        Self { password, fields }
    }

    /// First line of the entry — the password.
    pub fn password(&self) -> &str {
        &self.password
    }

    /// Look up a field by name. Matching is case-insensitive.
    pub fn field(&self, name: &str) -> Option<&str> {
        let needle = name.to_lowercase();
        self.fields
            .iter()
            .find(|(k, _)| *k == needle)
            .map(|(_, v)| v.as_str())
    }

    /// All `(key, value)` pairs in their original order. Keys are
    /// lowercased.
    pub fn fields(&self) -> &[(String, String)] {
        &self.fields
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_line_entry_has_only_a_password() {
        let e = Entry::parse(b"hunter2").unwrap();
        assert_eq!(e.password(), "hunter2");
        assert!(e.fields().is_empty());
        assert!(e.field("login").is_none());
    }

    #[test]
    fn multi_line_entry_extracts_fields() {
        let body = b"hunter2\nlogin: alice\nurl: https://example.com\n";
        let e = Entry::parse(body).unwrap();
        assert_eq!(e.password(), "hunter2");
        assert_eq!(e.field("login"), Some("alice"));
        assert_eq!(e.field("url"), Some("https://example.com"));
    }

    #[test]
    fn field_lookup_is_case_insensitive() {
        let e = Entry::parse(b"pw\nLogin: alice\nURL: https://x\n").unwrap();
        assert_eq!(e.field("login"), Some("alice"));
        assert_eq!(e.field("LOGIN"), Some("alice"));
        assert_eq!(e.field("Login"), Some("alice"));
        assert_eq!(e.field("uRl"), Some("https://x"));
    }

    #[test]
    fn whitespace_around_key_and_value_is_trimmed() {
        let e = Entry::parse(b"pw\n  login   :   alice  \n").unwrap();
        assert_eq!(e.field("login"), Some("alice"));
    }

    #[test]
    fn lines_without_a_colon_are_dropped_from_field_index() {
        let e = Entry::parse(b"pw\njust a note\nlogin: alice\n").unwrap();
        assert_eq!(e.field("login"), Some("alice"));
        assert_eq!(e.fields().len(), 1);
    }

    #[test]
    fn empty_lines_between_fields_are_ignored() {
        let e = Entry::parse(b"pw\n\nlogin: alice\n\n\nurl: x\n").unwrap();
        assert_eq!(e.field("login"), Some("alice"));
        assert_eq!(e.field("url"), Some("x"));
    }

    #[test]
    fn otpauth_uri_is_addressable_as_otpauth_field() {
        // Pass-otp relies on this: it stores the otpauth URI as a line
        // and finds it by looking up the `otpauth` field.
        let body = b"pw\notpauth://totp/Example:alice?secret=JBSWY3DPEHPK3PXP\n";
        let e = Entry::parse(body).unwrap();
        assert_eq!(
            e.field("otpauth"),
            Some("//totp/Example:alice?secret=JBSWY3DPEHPK3PXP")
        );
    }

    #[test]
    fn crlf_line_endings_are_handled() {
        let e = Entry::parse(b"pw\r\nlogin: alice\r\n").unwrap();
        assert_eq!(e.password(), "pw");
        assert_eq!(e.field("login"), Some("alice"));
    }

    #[test]
    fn non_utf8_input_is_rejected() {
        let err = Entry::parse(&[0xff, 0xfe, 0x00]).unwrap_err();
        assert!(matches!(err, EntryError::NotUtf8));
    }

    #[test]
    fn duplicate_fields_yield_first_match() {
        let e = Entry::parse(b"pw\nlogin: first\nlogin: second\n").unwrap();
        assert_eq!(e.field("login"), Some("first"));
        assert_eq!(e.fields().len(), 2);
    }

    #[test]
    fn empty_input_yields_empty_password() {
        let e = Entry::parse(b"").unwrap();
        assert_eq!(e.password(), "");
        assert!(e.fields().is_empty());
    }
}
