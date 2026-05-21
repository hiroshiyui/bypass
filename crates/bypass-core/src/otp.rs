// SPDX-License-Identifier: GPL-3.0-or-later

//! TOTP support: find an `otpauth://` URI inside a decrypted entry and
//! compute the current time-based one-time password.
//!
//! Pass-compatible convention (matching the `pass-otp` extension): the
//! `otpauth://…` URI lives on a line of its own anywhere in the entry
//! body. We scan the plaintext line-by-line and use the first match.
//! The URI is parsed with [`totp_rs::TOTP::from_url_unchecked`] —
//! `from_url` rejects URIs missing an `issuer`, which is overly strict
//! for stores migrated from other clients.

use totp_rs::TOTP;

#[derive(Debug, thiserror::Error)]
pub enum OtpError {
    #[error("entry does not contain an `otpauth://` URI")]
    NoOtpauthUri,

    #[error("failed to parse otpauth URI: {0}")]
    ParseUri(String),

    #[error("failed to compute TOTP code: {0}")]
    GenerateCode(String),
}

/// Find the `otpauth://` URI inside `plaintext` and return the current
/// TOTP code.
pub fn current_code(plaintext: &str) -> Result<String, OtpError> {
    let totp = parse(plaintext)?;
    totp.generate_current()
        .map_err(|e| OtpError::GenerateCode(e.to_string()))
}

/// Find and parse the `otpauth://` URI inside `plaintext`. Exposed so
/// callers (and tests) can compute codes for arbitrary timestamps via
/// `TOTP::generate(time)`.
pub fn parse(plaintext: &str) -> Result<TOTP, OtpError> {
    let uri = plaintext
        .lines()
        .map(|l| l.strip_suffix('\r').unwrap_or(l).trim())
        .find(|l| l.starts_with("otpauth://"))
        .ok_or(OtpError::NoOtpauthUri)?;
    TOTP::from_url_unchecked(uri).map_err(|e| OtpError::ParseUri(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    // RFC 6238 test vector secret. Base32("12345678901234567890") =
    // "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ".
    const TEST_URI: &str =
        "otpauth://totp/Example:alice?secret=GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ&issuer=Example";

    #[test]
    fn extracts_otpauth_line_from_multi_line_entry() {
        let body = format!("hunter2\nlogin: alice\n{TEST_URI}\nurl: https://x\n");
        let code = current_code(&body).unwrap();
        assert_eq!(code.len(), 6);
        assert!(
            code.chars().all(|c| c.is_ascii_digit()),
            "code is not all digits: {code}"
        );
    }

    #[test]
    fn accepts_otpauth_as_the_only_line() {
        let code = current_code(TEST_URI).unwrap();
        assert_eq!(code.len(), 6);
    }

    #[test]
    fn rfc6238_test_vector_matches_at_known_time() {
        // RFC 6238 Appendix B (SHA-1 column): at T = 59 the truncated
        // 8-digit code is 94287082, so the 6-digit code is 287082.
        let totp = parse(TEST_URI).unwrap();
        assert_eq!(totp.generate(59), "287082");
    }

    #[test]
    fn entry_without_otpauth_returns_no_uri_error() {
        let body = "hunter2\nlogin: alice\nurl: https://x\n";
        let err = current_code(body).unwrap_err();
        assert!(matches!(err, OtpError::NoOtpauthUri));
    }

    #[test]
    fn malformed_otpauth_uri_is_rejected() {
        let body = "pw\notpauth://totp/Example?garbage\n";
        let err = current_code(body).unwrap_err();
        assert!(matches!(err, OtpError::ParseUri(_)));
    }

    #[test]
    fn crlf_line_endings_are_tolerated() {
        let body = format!("pw\r\n{TEST_URI}\r\n");
        let code = current_code(&body).unwrap();
        assert_eq!(code.len(), 6);
    }

    #[test]
    fn whitespace_around_uri_line_is_tolerated() {
        let body = format!("pw\n  {TEST_URI}  \n");
        let code = current_code(&body).unwrap();
        assert_eq!(code.len(), 6);
    }
}
