// SPDX-License-Identifier: GPL-3.0-or-later

//! [`Crypto`] implementation backed by the system `gpg` binary.
//!
//! The CLI shells out to `gpg` rather than linking a Rust OpenPGP library
//! so that we inherit the user's existing keyring, `gpg-agent` policy, and
//! smartcard / hardware-token integrations — see
//! [ADR-0001](../../../doc/adr/0001-platform-delegated-crypto.md).
//!
//! This module is intentionally narrow: spawn `gpg` for each call, pipe
//! plaintext or ciphertext through stdin, capture stdout as the result.
//! No persistent process, no global state.

use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

use bypass_core::crypto::{Crypto, KeyId, SecretBytes};

/// Failures that can occur while invoking `gpg`.
#[derive(Debug, thiserror::Error)]
pub enum GpgError {
    #[error("failed to spawn `{binary}`: {source}")]
    Spawn {
        binary: String,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to write to gpg stdin: {0}")]
    Stdin(#[source] std::io::Error),

    #[error("failed to wait on gpg: {0}")]
    Wait(#[source] std::io::Error),

    #[error("gpg exited with status {status}: {stderr}")]
    NonZero { status: i32, stderr: String },
}

/// `Crypto` implementation that shells out to the system `gpg` binary.
#[derive(Debug, Clone)]
pub struct GpgCli {
    binary: OsString,
    homedir: Option<PathBuf>,
}

impl Default for GpgCli {
    fn default() -> Self {
        Self {
            binary: OsString::from("gpg"),
            homedir: None,
        }
    }
}

impl GpgCli {
    /// Use the default `gpg` binary on `PATH` and the user's default
    /// `GNUPGHOME`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Override the `GNUPGHOME` directory passed to `gpg --homedir`.
    /// Primarily for tests against a throwaway keyring.
    #[allow(dead_code)] // used in tests; main.rs uses the default homedir
    pub fn with_homedir(mut self, homedir: PathBuf) -> Self {
        self.homedir = Some(homedir);
        self
    }

    fn base_command(&self) -> Command {
        let mut cmd = Command::new(&self.binary);
        if let Some(home) = &self.homedir {
            cmd.arg("--homedir").arg(home);
        }
        cmd.arg("--batch")
            .arg("--no-tty")
            .arg("--quiet")
            .arg("--yes");
        cmd
    }

    fn binary_for_error(&self) -> String {
        self.binary.to_string_lossy().into_owned()
    }

    /// Spawn `gpg --encrypt --recipient <recipient>` with piped
    /// stdin (caller writes the tar bytes), piped stdout (caller
    /// reads the ciphertext), and piped stderr. Used by `bypass
    /// backup` to stream a tar bundle through GPG without ever
    /// buffering the full plaintext in memory (ADR-0026 / Milestone
    /// 4.4). The caller owns thread orchestration: typically a
    /// background thread writes to `child.stdin` while the main
    /// thread reads from `child.stdout`, then both rendezvous on
    /// `child.wait()`. Stderr is captured for the error message if
    /// gpg exits non-zero — see [`drain_stderr_and_status`].
    pub fn spawn_encrypt_stream(&self, recipient: &str) -> Result<Child, GpgError> {
        let mut cmd = self.base_command();
        cmd.arg("--trust-model")
            .arg("always")
            .arg("--output")
            .arg("-")
            .arg("--encrypt")
            .arg("--recipient")
            .arg(recipient);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd.spawn().map_err(|source| GpgError::Spawn {
            binary: self.binary_for_error(),
            source,
        })
    }

    /// Spawn `gpg --decrypt` with piped stdin (caller pipes the
    /// outer-wrapped tar in), piped stdout (caller reads the
    /// recovered tar out), and piped stderr. Counterpart to
    /// [`spawn_encrypt_stream`]; used by `bypass restore`.
    pub fn spawn_decrypt_stream(&self) -> Result<Child, GpgError> {
        let mut cmd = self.base_command();
        cmd.arg("--output").arg("-").arg("--decrypt");
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd.spawn().map_err(|source| GpgError::Spawn {
            binary: self.binary_for_error(),
            source,
        })
    }

    fn run(&self, mut cmd: Command, stdin_data: &[u8]) -> Result<Vec<u8>, GpgError> {
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|source| GpgError::Spawn {
            binary: self.binary_for_error(),
            source,
        })?;
        {
            let stdin = child
                .stdin
                .as_mut()
                .expect("stdin requested via Stdio::piped");
            stdin.write_all(stdin_data).map_err(GpgError::Stdin)?;
        }
        let output = child.wait_with_output().map_err(GpgError::Wait)?;
        if !output.status.success() {
            return Err(GpgError::NonZero {
                status: output.status.code().unwrap_or(-1),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
        }
        Ok(output.stdout)
    }
}

/// Wait for `child` and convert a non-zero exit into a
/// [`GpgError::NonZero`] with the captured stderr. Used by the
/// streaming `spawn_*_stream` consumers (`backup`, `restore`) once
/// they're done pumping stdin/stdout.
pub fn finish_streaming(mut child: Child) -> Result<(), GpgError> {
    use std::io::Read;
    let mut stderr_buf = Vec::new();
    if let Some(mut s) = child.stderr.take() {
        let _ = s.read_to_end(&mut stderr_buf);
    }
    let status = child.wait().map_err(GpgError::Wait)?;
    if !status.success() {
        return Err(GpgError::NonZero {
            status: status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&stderr_buf).into_owned(),
        });
    }
    Ok(())
}

impl Crypto for GpgCli {
    type Error = GpgError;

    fn encrypt(&self, plaintext: &[u8], recipients: &[KeyId]) -> Result<Vec<u8>, Self::Error> {
        let mut cmd = self.base_command();
        // `--trust-model always` mirrors `pass`: a recipient key listed in
        // `.gpg-id` should be honoured even if it isn't marked ultimately
        // trusted in the user's keyring.
        cmd.arg("--trust-model")
            .arg("always")
            .arg("--output")
            .arg("-")
            .arg("--encrypt");
        for r in recipients {
            cmd.arg("--recipient").arg(r.as_str());
        }
        self.run(cmd, plaintext)
    }

    fn decrypt(&self, ciphertext: &[u8]) -> Result<SecretBytes, Self::Error> {
        let mut cmd = self.base_command();
        cmd.arg("--output").arg("-").arg("--decrypt");
        let plaintext = self.run(cmd, ciphertext)?;
        Ok(SecretBytes::new(plaintext))
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::process::Command;

    use tempfile::TempDir;

    use super::*;

    const TEST_USER_ID: &str = "bypass-test <bypass-test@example.invalid>";

    /// Build a throwaway GNUPGHOME with one passwordless key and return it.
    ///
    /// The directory is wiped when the returned `TempDir` drops, so each
    /// test gets a completely isolated keyring. This never touches
    /// `~/.gnupg` — see CLAUDE.md.
    fn fresh_gnupghome() -> TempDir {
        let home = TempDir::new().expect("create tempdir");
        // gpg refuses to use a world-readable homedir; tempfile usually
        // already chmods 0700, but make it explicit on Unix.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(home.path(), fs::Permissions::from_mode(0o700))
                .expect("chmod 0700");
        }
        // Allow `--pinentry-mode loopback` so passphrase-less key gen does
        // not get blocked by a pinentry policy.
        fs::write(
            home.path().join("gpg-agent.conf"),
            "allow-loopback-pinentry\n",
        )
        .expect("write gpg-agent.conf");

        let status = Command::new("gpg")
            .arg("--homedir")
            .arg(home.path())
            .arg("--batch")
            .arg("--no-tty")
            .arg("--quiet")
            .arg("--pinentry-mode")
            .arg("loopback")
            .arg("--passphrase")
            .arg("")
            .arg("--quick-generate-key")
            .arg(TEST_USER_ID)
            .arg("default")
            .arg("default")
            .arg("0")
            .status()
            .expect("spawn gpg for key generation");
        assert!(status.success(), "gpg key generation failed: {status}");
        home
    }

    fn killing_agent(home: &Path) {
        // Best-effort: shut down the agent we spawned so the tempdir can be
        // unlinked cleanly. Ignore failures — the OS will reap the agent
        // when its socket is gone.
        let _ = Command::new("gpgconf")
            .arg("--homedir")
            .arg(home)
            .arg("--kill")
            .arg("gpg-agent")
            .status();
    }

    #[test]
    fn roundtrip_encrypt_then_decrypt() {
        let home = fresh_gnupghome();
        let gpg = GpgCli::new().with_homedir(home.path().to_path_buf());

        let plaintext = b"hunter2\nlogin: alice\n";
        let ciphertext = gpg
            .encrypt(plaintext, &[KeyId::new(TEST_USER_ID)])
            .expect("encrypt");
        assert_ne!(
            ciphertext, plaintext,
            "ciphertext must differ from plaintext"
        );
        assert!(
            ciphertext.len() > plaintext.len(),
            "ciphertext should be larger than plaintext"
        );

        let recovered = gpg.decrypt(&ciphertext).expect("decrypt");
        assert_eq!(recovered.as_slice(), plaintext);

        killing_agent(home.path());
    }

    #[test]
    fn decrypt_garbage_returns_nonzero_error() {
        let home = fresh_gnupghome();
        let gpg = GpgCli::new().with_homedir(home.path().to_path_buf());

        let err = gpg.decrypt(b"this is not openpgp data").unwrap_err();
        match err {
            GpgError::NonZero { status, stderr } => {
                assert_ne!(status, 0);
                assert!(!stderr.is_empty(), "expected gpg to log to stderr");
            }
            other => panic!("expected NonZero, got {other:?}"),
        }

        killing_agent(home.path());
    }

    #[test]
    fn encrypt_to_unknown_recipient_fails() {
        let home = fresh_gnupghome();
        let gpg = GpgCli::new().with_homedir(home.path().to_path_buf());

        let err = gpg
            .encrypt(b"x", &[KeyId::new("nobody@example.invalid")])
            .unwrap_err();
        assert!(matches!(err, GpgError::NonZero { .. }));

        killing_agent(home.path());
    }

    #[test]
    fn spawn_failure_reports_binary_name() {
        let gpg = GpgCli {
            binary: "definitely-not-a-real-binary-xyz".into(),
            homedir: None,
        };
        let err = gpg.encrypt(b"x", &[KeyId::new("anyone")]).unwrap_err();
        match err {
            GpgError::Spawn { binary, .. } => {
                assert!(binary.contains("definitely-not-a-real-binary-xyz"));
            }
            other => panic!("expected Spawn, got {other:?}"),
        }
    }
}
