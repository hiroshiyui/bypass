// SPDX-License-Identifier: GPL-3.0-or-later

//! Shared helpers for integration tests under `tests/`.
//!
//! Builds a throwaway `GNUPGHOME` with a passphrase-less ed25519 key and a
//! tempdir to use as `PASSWORD_STORE_DIR`. Drop tears the agent down so
//! the homedir can be unlinked cleanly.

use std::fs;
use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

pub const TEST_USER_ID: &str = "bypass-it <bypass-it@example.invalid>";
// Not every integration-test binary needs this constant (each `tests/*.rs`
// is its own crate that pulls in `common`), so silence dead-code per-test.
#[allow(dead_code)]
pub const TEST_RECIPIENT: &str = "bypass-it@example.invalid";

pub struct TestEnv {
    pub gnupghome: TempDir,
    pub store_dir: TempDir,
}

impl TestEnv {
    pub fn new() -> Self {
        let gnupghome = fresh_gnupghome();
        let store_dir = TempDir::new().expect("create store tempdir");
        Self {
            gnupghome,
            store_dir,
        }
    }

    /// Env pairs to inject into spawned `bypass` invocations. We don't
    /// `env_clear()` because the binary still needs `PATH` (for `gpg`)
    /// and locale.
    pub fn env_pairs(&self) -> Vec<(&'static str, &Path)> {
        vec![
            ("GNUPGHOME", self.gnupghome.path()),
            ("PASSWORD_STORE_DIR", self.store_dir.path()),
        ]
    }
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        let _ = Command::new("gpgconf")
            .arg("--homedir")
            .arg(self.gnupghome.path())
            .arg("--kill")
            .arg("gpg-agent")
            .status();
    }
}

fn fresh_gnupghome() -> TempDir {
    let home = TempDir::new().expect("create gnupghome tempdir");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(home.path(), fs::Permissions::from_mode(0o700)).expect("chmod 0700");
    }
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
