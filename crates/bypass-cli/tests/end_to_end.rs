// SPDX-License-Identifier: GPL-3.0-or-later

//! End-to-end integration test for the `bypass` binary.
//!
//! Drives the compiled binary (`assert_cmd::Command::cargo_bin`) against a
//! throwaway `GNUPGHOME` + tempdir password store, exercising the full
//! Milestone 1.3 surface: `doctor`, `init`, `insert`, `show`, `ls`,
//! `find`, `cp`, `mv`, `rm`. `edit` is exercised separately so we can
//! drive a non-interactive editor stub through `$EDITOR`.

use assert_cmd::Command;
use predicates::prelude::*;

mod common;

fn bypass(env: &common::TestEnv) -> Command {
    let mut cmd = Command::cargo_bin("bypass").expect("cargo_bin");
    for (k, v) in env.env_pairs() {
        cmd.env(k, v);
    }
    cmd
}

#[test]
fn full_crud_flow() {
    let env = common::TestEnv::new();

    // Doctor before init: store dir exists (tempdir), but .gpg-id is
    // missing, so the recipients check fails → exit 1.
    bypass(&env).arg("doctor").assert().failure();

    // init writes .gpg-id and (now backed by Git2Vcs) creates a repo.
    bypass(&env)
        .arg("init")
        .arg(common::TEST_RECIPIENT)
        .assert()
        .success();

    bypass(&env).arg("doctor").assert().success();

    // insert via pipe (non-TTY → single-line read).
    bypass(&env)
        .args(["insert", "email/work"])
        .write_stdin("hunter2")
        .assert()
        .success();
    bypass(&env)
        .args(["insert", "email/personal"])
        .write_stdin("p3rs0nal")
        .assert()
        .success();
    bypass(&env)
        .args(["insert", "bank/visa"])
        .write_stdin("4111-1111-1111-1111")
        .assert()
        .success();

    // show roundtrips the plaintext.
    bypass(&env)
        .args(["show", "email/work"])
        .assert()
        .success()
        .stdout(predicate::str::starts_with("hunter2"));

    // ls renders a tree containing every inserted entry.
    bypass(&env)
        .arg("ls")
        .assert()
        .success()
        .stdout(predicate::str::contains("email"))
        .stdout(predicate::str::contains("work"))
        .stdout(predicate::str::contains("personal"))
        .stdout(predicate::str::contains("bank"))
        .stdout(predicate::str::contains("visa"));

    // find by substring.
    bypass(&env)
        .args(["find", "email"])
        .assert()
        .success()
        .stdout(predicate::str::contains("email/personal"))
        .stdout(predicate::str::contains("email/work"));

    // cp within the same recipient set (byte-copy under the hood).
    bypass(&env)
        .args(["cp", "email/work", "email/work-backup"])
        .assert()
        .success();
    bypass(&env)
        .args(["show", "email/work-backup"])
        .assert()
        .success()
        .stdout(predicate::str::starts_with("hunter2"));

    // mv (rename within store).
    bypass(&env)
        .args(["mv", "email/work-backup", "archive/work"])
        .assert()
        .success();
    bypass(&env)
        .args(["show", "archive/work"])
        .assert()
        .success()
        .stdout(predicate::str::starts_with("hunter2"));
    bypass(&env)
        .args(["show", "email/work-backup"])
        .assert()
        .failure();

    // rm (single).
    bypass(&env).args(["rm", "archive/work"]).assert().success();
    bypass(&env)
        .args(["show", "archive/work"])
        .assert()
        .failure();

    // rm -r (subtree).
    bypass(&env)
        .args(["rm", "--recursive", "bank"])
        .assert()
        .success();
    bypass(&env).args(["show", "bank/visa"]).assert().failure();

    // Final ls: only email/ entries remain.
    bypass(&env)
        .arg("ls")
        .assert()
        .success()
        .stdout(predicate::str::contains("email"))
        .stdout(predicate::str::contains("personal"))
        .stdout(predicate::str::contains("work"))
        .stdout(predicate::str::contains("bank").not())
        .stdout(predicate::str::contains("archive").not());
}

#[test]
fn edit_persists_changes_through_an_external_editor() {
    let env = common::TestEnv::new();
    bypass(&env)
        .arg("init")
        .arg(common::TEST_RECIPIENT)
        .assert()
        .success();
    bypass(&env)
        .args(["insert", "note"])
        .write_stdin("v1")
        .assert()
        .success();

    // Use a shell snippet as the editor: append a line to the tempfile.
    // sh's "$0" is the path argument passed by bypass's `sh -c '<editor>
    // <path>'` invocation. The single quotes in our wrapper keep the
    // shell layers tidy.
    bypass(&env)
        .args(["edit", "note"])
        .env("EDITOR", "sh -c 'printf appended >> \"$0\"'")
        .assert()
        .success();

    bypass(&env)
        .args(["show", "note"])
        .assert()
        .success()
        .stdout(predicate::str::starts_with("v1appended"));
}

#[test]
fn init_creates_a_git_repo_and_inserts_auto_commit() {
    let env = common::TestEnv::new();
    bypass(&env)
        .arg("init")
        .arg(common::TEST_RECIPIENT)
        .assert()
        .success();

    // After init the store directory must contain a real git repo.
    assert!(
        env.store_dir.path().join(".git").is_dir(),
        "init must create .git/ under the store root"
    );

    bypass(&env)
        .args(["insert", "email/work"])
        .write_stdin("hunter2")
        .assert()
        .success();
    bypass(&env).args(["rm", "email/work"]).assert().success();

    // `bypass git log --oneline` should now show three commits: init,
    // insert, remove. Run it through the passthrough subcommand so we
    // exercise that code path too.
    let out = bypass(&env)
        .args(["git", "log", "--oneline"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&out.get_output().stdout).into_owned();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 3, "expected 3 commits, got:\n{stdout}");
    assert!(
        lines[0].contains("Remove email/work"),
        "head commit was {:?}",
        lines[0]
    );
    assert!(
        lines[1].contains("Add password for email/work"),
        "second commit was {:?}",
        lines[1]
    );
    assert!(
        lines[2].contains("initialise store"),
        "root commit was {:?}",
        lines[2]
    );
}

#[test]
fn log_shows_full_and_filtered_history() {
    let env = common::TestEnv::new();
    bypass(&env)
        .arg("init")
        .arg(common::TEST_RECIPIENT)
        .assert()
        .success();
    bypass(&env)
        .args(["insert", "email/work"])
        .write_stdin("p1")
        .assert()
        .success();
    bypass(&env)
        .args(["insert", "bank/visa"])
        .write_stdin("p2")
        .assert()
        .success();

    // Full log: 3 commits (init + two inserts). Same-second timestamps
    // make intra-second ordering implementation-defined, so we assert
    // membership rather than exact sequence.
    let out = bypass(&env).arg("log").assert().success();
    let stdout = String::from_utf8_lossy(&out.get_output().stdout).into_owned();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 3, "full log was:\n{stdout}");
    assert!(
        lines
            .iter()
            .any(|l| l.contains("Add password for bank/visa")),
        "expected bank/visa commit, got:\n{stdout}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("Add password for email/work")),
        "expected email/work commit, got:\n{stdout}"
    );
    assert!(
        lines.iter().any(|l| l.contains("initialise store")),
        "expected init commit, got:\n{stdout}"
    );

    // Path-filtered log: only the bank/visa commit (init touches
    // `.gpg-id`, not `bank/visa.gpg`).
    let out = bypass(&env).args(["log", "bank/visa"]).assert().success();
    let stdout = String::from_utf8_lossy(&out.get_output().stdout).into_owned();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 1, "filtered log was:\n{stdout}");
    assert!(lines[0].contains("Add password for bank/visa"));
}

#[test]
fn mutations_refuse_when_repo_is_mid_merge() {
    let env = common::TestEnv::new();
    bypass(&env)
        .arg("init")
        .arg(common::TEST_RECIPIENT)
        .assert()
        .success();
    bypass(&env)
        .args(["insert", "note"])
        .write_stdin("v1")
        .assert()
        .success();

    // Drop a MERGE_HEAD marker so the next mutation thinks the repo is
    // mid-merge and refuses. Use git rev-parse via the passthrough so we
    // don't depend on libgit2 from a test.
    let head_oid = std::process::Command::new("git")
        .args(["-C"])
        .arg(env.store_dir.path())
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("git rev-parse")
        .stdout;
    let head_oid = String::from_utf8(head_oid).unwrap().trim().to_owned();
    std::fs::write(env.store_dir.path().join(".git/MERGE_HEAD"), &head_oid).unwrap();

    bypass(&env)
        .args(["insert", "other"])
        .write_stdin("v2")
        .assert()
        .failure()
        .stderr(predicate::str::contains("merge"));
}

#[test]
fn generate_stores_and_prints_password_of_requested_length() {
    let env = common::TestEnv::new();
    bypass(&env)
        .arg("init")
        .arg(common::TEST_RECIPIENT)
        .assert()
        .success();

    let out = bypass(&env)
        .args(["generate", "wifi", "32", "--no-symbols"])
        .assert()
        .success();
    let printed = String::from_utf8_lossy(&out.get_output().stdout)
        .trim()
        .to_owned();
    assert_eq!(printed.chars().count(), 32);
    assert!(
        printed.chars().all(|c| c.is_ascii_alphanumeric()),
        "--no-symbols password contained non-alphanumeric: {printed}"
    );

    // The same password must roundtrip through `show`.
    bypass(&env)
        .args(["show", "wifi"])
        .assert()
        .success()
        .stdout(predicate::str::starts_with(printed.as_str()));
}

#[test]
fn generate_without_force_refuses_to_overwrite() {
    let env = common::TestEnv::new();
    bypass(&env)
        .arg("init")
        .arg(common::TEST_RECIPIENT)
        .assert()
        .success();
    bypass(&env)
        .args(["insert", "wifi"])
        .write_stdin("preexisting")
        .assert()
        .success();
    bypass(&env)
        .args(["generate", "wifi"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("already exists"));
    bypass(&env)
        .args(["show", "wifi"])
        .assert()
        .success()
        .stdout(predicate::str::starts_with("preexisting"));
}

#[test]
fn generate_in_place_preserves_trailing_lines() {
    let env = common::TestEnv::new();
    bypass(&env)
        .arg("init")
        .arg(common::TEST_RECIPIENT)
        .assert()
        .success();
    // Multi-line entry: first line is the password, rest is metadata in the
    // pass `key: value` style.
    bypass(&env)
        .args(["insert", "--multiline", "service"])
        .write_stdin("old-password\nlogin: alice\nurl: https://example.com\n")
        .assert()
        .success();

    let out = bypass(&env)
        .args(["generate", "service", "16", "--in-place"])
        .assert()
        .success();
    let new_password = String::from_utf8_lossy(&out.get_output().stdout)
        .trim()
        .to_owned();
    assert_eq!(new_password.chars().count(), 16);

    let shown = bypass(&env).args(["show", "service"]).assert().success();
    let stdout = String::from_utf8_lossy(&shown.get_output().stdout).into_owned();
    let mut lines = stdout.lines();
    assert_eq!(lines.next().unwrap(), new_password);
    assert_eq!(lines.next().unwrap(), "login: alice");
    assert_eq!(lines.next().unwrap(), "url: https://example.com");
}

#[test]
fn show_with_field_arg_prints_only_that_field() {
    let env = common::TestEnv::new();
    bypass(&env)
        .arg("init")
        .arg(common::TEST_RECIPIENT)
        .assert()
        .success();
    bypass(&env)
        .args(["insert", "--multiline", "service"])
        .write_stdin("hunter2\nlogin: alice\nurl: https://example.com\n")
        .assert()
        .success();

    // No field → full entry.
    bypass(&env)
        .args(["show", "service"])
        .assert()
        .success()
        .stdout(predicate::str::contains("hunter2"))
        .stdout(predicate::str::contains("login: alice"));

    // With field → just the value.
    bypass(&env)
        .args(["show", "service", "login"])
        .assert()
        .success()
        .stdout(predicate::str::starts_with("alice"))
        .stdout(predicate::str::contains("hunter2").not())
        .stdout(predicate::str::contains("https://").not());

    // Field lookup is case-insensitive.
    bypass(&env)
        .args(["show", "service", "URL"])
        .assert()
        .success()
        .stdout(predicate::str::starts_with("https://example.com"));

    // Missing field is an error.
    bypass(&env)
        .args(["show", "service", "ghost"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no field"));
}

#[test]
fn otp_prints_six_digit_code_from_otpauth_uri() {
    let env = common::TestEnv::new();
    bypass(&env)
        .arg("init")
        .arg(common::TEST_RECIPIENT)
        .assert()
        .success();
    // RFC 6238 test vector secret.
    bypass(&env)
        .args(["insert", "--multiline", "totp/example"])
        .write_stdin(concat!(
            "hunter2\n",
            "login: alice\n",
            "otpauth://totp/Example:alice?secret=GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ&issuer=Example\n",
        ))
        .assert()
        .success();

    let out = bypass(&env)
        .args(["otp", "totp/example"])
        .assert()
        .success();
    let code = String::from_utf8_lossy(&out.get_output().stdout)
        .trim()
        .to_owned();
    assert_eq!(code.len(), 6, "OTP output was {code:?}");
    assert!(
        code.chars().all(|c| c.is_ascii_digit()),
        "OTP output is not all digits: {code}"
    );
}

#[test]
fn otp_without_otpauth_uri_fails_with_helpful_message() {
    let env = common::TestEnv::new();
    bypass(&env)
        .arg("init")
        .arg(common::TEST_RECIPIENT)
        .assert()
        .success();
    bypass(&env)
        .args(["insert", "no-otp"])
        .write_stdin("just a password")
        .assert()
        .success();
    bypass(&env)
        .args(["otp", "no-otp"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("otpauth"));
}

#[test]
fn init_writes_gitattributes_with_gpg_binary_rule() {
    let env = common::TestEnv::new();
    bypass(&env)
        .arg("init")
        .arg(common::TEST_RECIPIENT)
        .assert()
        .success();
    let body = std::fs::read_to_string(env.store_dir.path().join(".gitattributes"))
        .expect(".gitattributes was written");
    assert!(body.contains("*.gpg binary"), ".gitattributes was {body:?}");
}

#[test]
#[cfg(unix)]
fn sync_lazily_installs_gitattributes_on_legacy_stores() {
    let env = common::TestEnv::new();
    let remote = bare_remote();
    bypass(&env)
        .arg("init")
        .arg(common::TEST_RECIPIENT)
        .assert()
        .success();

    // Simulate a legacy store created before the auto-install: drop
    // `.gitattributes` from disk *and* from the git index, then commit
    // its removal. From this point the store is in the shape an
    // upgraded user would be in.
    let attrs = env.store_dir.path().join(".gitattributes");
    assert!(attrs.exists(), "init should have written .gitattributes");
    bypass(&env)
        .args(["git", "rm", ".gitattributes"])
        .assert()
        .success();
    bypass(&env)
        .args([
            "git",
            "commit",
            "-m",
            "drop .gitattributes (simulating legacy store)",
        ])
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .assert()
        .success();
    assert!(!attrs.exists(), "removal didn't take effect");

    let branch = current_branch(&env);
    bypass(&env)
        .args(["git", "remote", "add", "origin"])
        .arg(remote.path())
        .assert()
        .success();
    bypass(&env)
        .args(["git", "push", "-u", "origin", &branch])
        .assert()
        .success();

    bypass(&env).arg("sync").assert().success();

    // The lazy install should have re-created the file and committed it.
    let body = std::fs::read_to_string(&attrs).expect("sync should have restored .gitattributes");
    assert!(body.contains("*.gpg binary"), ".gitattributes was {body:?}");

    // Verify the commit is real and reached the remote.
    let log = std::process::Command::new("git")
        .arg("-C")
        .arg(env.store_dir.path())
        .args(["log", "--oneline", "-1"])
        .output()
        .unwrap();
    let head_summary = String::from_utf8(log.stdout).unwrap();
    assert!(
        head_summary.contains("install .gitattributes"),
        "head commit was {head_summary:?}"
    );
}

#[test]
#[cfg(unix)]
fn ext_runs_an_extension_and_passes_env_vars() {
    use std::os::unix::fs::PermissionsExt;

    let env = common::TestEnv::new();
    bypass(&env)
        .arg("init")
        .arg(common::TEST_RECIPIENT)
        .assert()
        .success();

    // Place a tiny shell-script extension in a per-test directory and
    // point PASSWORD_STORE_EXTENSIONS_DIR at it.
    let ext_dir = tempfile::TempDir::new().unwrap();
    let script_path = ext_dir.path().join("dump-env");
    std::fs::write(
        &script_path,
        b"#!/bin/sh\nprintf 'STORE=%s\\nBIN=%s\\nARGS=%s\\n' \
            \"$PASSWORD_STORE_DIR\" \"$PASSWORD_STORE_BIN\" \"$*\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();

    let store_path = env.store_dir.path().to_path_buf();
    let out = bypass(&env)
        .env("PASSWORD_STORE_EXTENSIONS_DIR", ext_dir.path())
        .args(["ext", "dump-env", "hello", "world"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&out.get_output().stdout).into_owned();
    assert!(
        stdout.contains(&format!("STORE={}", store_path.display())),
        "expected STORE in output, got:\n{stdout}"
    );
    assert!(stdout.contains("BIN="), "expected BIN= line:\n{stdout}");
    assert!(stdout.contains("ARGS=hello world"));
}

#[test]
#[cfg(unix)]
fn ext_without_exec_bit_is_not_found() {
    let env = common::TestEnv::new();
    bypass(&env)
        .arg("init")
        .arg(common::TEST_RECIPIENT)
        .assert()
        .success();
    let ext_dir = tempfile::TempDir::new().unwrap();
    std::fs::write(ext_dir.path().join("not-exec"), b"#!/bin/sh\necho hi\n").unwrap();
    // chmod 0644 — readable but not executable.
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(
        ext_dir.path().join("not-exec"),
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();

    bypass(&env)
        .env("PASSWORD_STORE_EXTENSIONS_DIR", ext_dir.path())
        .args(["ext", "not-exec"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not found"));
}

/// Initialise a bare git repo to act as the remote for sync tests.
#[cfg(unix)]
fn bare_remote() -> tempfile::TempDir {
    let td = tempfile::TempDir::new().unwrap();
    let status = std::process::Command::new("git")
        .arg("init")
        .arg("--bare")
        .arg(td.path())
        .status()
        .expect("git init --bare");
    assert!(status.success(), "git init --bare failed");
    td
}

/// Helper: short-hand for the current local branch name (whatever
/// `init.defaultBranch` is on the test host — usually `main`).
#[cfg(unix)]
fn current_branch(env: &common::TestEnv) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(env.store_dir.path())
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .expect("git rev-parse");
    assert!(out.status.success(), "git rev-parse HEAD failed");
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

#[test]
#[cfg(unix)]
fn sync_pushes_and_pulls_against_a_local_bare_remote() {
    let env = common::TestEnv::new();
    let remote = bare_remote();
    bypass(&env)
        .arg("init")
        .arg(common::TEST_RECIPIENT)
        .assert()
        .success();
    bypass(&env)
        .args(["insert", "email/work"])
        .write_stdin("hunter2")
        .assert()
        .success();

    let branch = current_branch(&env);
    bypass(&env)
        .args(["git", "remote", "add", "origin"])
        .arg(remote.path())
        .assert()
        .success();
    bypass(&env)
        .args(["git", "push", "-u", "origin", &branch])
        .assert()
        .success();

    bypass(&env)
        .args(["insert", "email/personal"])
        .write_stdin("p3rs0nal")
        .assert()
        .success();

    bypass(&env).arg("sync").assert().success();

    // The bare repo's ref should now match the local HEAD.
    let local_head = String::from_utf8(
        std::process::Command::new("git")
            .arg("-C")
            .arg(env.store_dir.path())
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let remote_head = String::from_utf8(
        std::process::Command::new("git")
            .arg("-C")
            .arg(remote.path())
            .args(["rev-parse", &branch])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert_eq!(local_head.trim(), remote_head.trim());

    // Second sync: no-op (nothing to push, nothing to pull).
    bypass(&env).arg("sync").assert().success();
}

#[test]
#[cfg(unix)]
fn sync_refuses_when_plaintext_is_staged() {
    let env = common::TestEnv::new();
    let remote = bare_remote();
    bypass(&env)
        .arg("init")
        .arg(common::TEST_RECIPIENT)
        .assert()
        .success();
    bypass(&env)
        .args(["insert", "email/work"])
        .write_stdin("hunter2")
        .assert()
        .success();
    let branch = current_branch(&env);
    bypass(&env)
        .args(["git", "remote", "add", "origin"])
        .arg(remote.path())
        .assert()
        .success();
    bypass(&env)
        .args(["git", "push", "-u", "origin", &branch])
        .assert()
        .success();

    // Stash plaintext into the store and commit it.
    std::fs::write(env.store_dir.path().join("notes.txt"), "real secret").unwrap();
    bypass(&env)
        .args(["git", "add", "notes.txt"])
        .assert()
        .success();
    bypass(&env)
        .args(["git", "commit", "-m", "oops"])
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .assert()
        .success();

    let before = String::from_utf8(
        std::process::Command::new("git")
            .arg("-C")
            .arg(remote.path())
            .args(["rev-parse", &branch])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();

    bypass(&env)
        .arg("sync")
        .assert()
        .failure()
        .stderr(predicate::str::contains("notes.txt"))
        .stderr(predicate::str::contains("suspicious"));

    let after = String::from_utf8(
        std::process::Command::new("git")
            .arg("-C")
            .arg(remote.path())
            .args(["rev-parse", &branch])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert_eq!(
        before, after,
        "remote ref must not advance when sync refuses"
    );

    // --force overrides and publishes.
    bypass(&env).args(["sync", "--force"]).assert().success();
    let after_force = String::from_utf8(
        std::process::Command::new("git")
            .arg("-C")
            .arg(remote.path())
            .args(["rev-parse", &branch])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert_ne!(before, after_force, "--force must advance the remote ref");
}

#[test]
#[cfg(unix)]
fn audit_lists_problem_files() {
    let env = common::TestEnv::new();
    bypass(&env)
        .arg("init")
        .arg(common::TEST_RECIPIENT)
        .assert()
        .success();
    bypass(&env)
        .args(["insert", "email/work"])
        .write_stdin("hunter2")
        .assert()
        .success();

    // Clean store before any plaintext is added.
    bypass(&env)
        .arg("audit")
        .assert()
        .success()
        .stderr(predicate::str::contains("clean"));

    // Add plaintext and commit.
    std::fs::write(env.store_dir.path().join("notes.txt"), "real secret").unwrap();
    bypass(&env)
        .args(["git", "add", "notes.txt"])
        .assert()
        .success();
    bypass(&env)
        .args(["git", "commit", "-m", "oops"])
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .assert()
        .success();

    bypass(&env)
        .arg("audit")
        .assert()
        .failure()
        .stdout(predicate::str::contains("notes.txt"))
        .stdout(predicate::str::contains("unknown filename"));
}

#[test]
fn sync_identity_rotate_requires_confirm_and_creates_a_key() {
    let env = common::TestEnv::new();
    // Point the CLI's identity-key resolver at an isolated config dir
    // so we never touch the developer's real ~/.config/bypass/.
    let cfg = tempfile::TempDir::new().unwrap();

    // Without --confirm: refuses.
    bypass(&env)
        .args(["sync", "identity", "rotate"])
        .env("XDG_CONFIG_HOME", cfg.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("--confirm"));

    // With --confirm: rotates. The identity file appears with mode 0600.
    bypass(&env)
        .args(["sync", "identity", "rotate", "--confirm"])
        .env("XDG_CONFIG_HOME", cfg.path())
        .assert()
        .success()
        .stderr(predicate::str::contains("New peer id"));

    let key_path = cfg.path().join("bypass").join("identity.key");
    assert!(
        key_path.exists(),
        "identity.key was not created at {}",
        key_path.display()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&key_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "wrote with mode {mode:#o}");
    }
}

#[test]
fn sync_pair_enter_without_addr_fails_with_helpful_message() {
    // The show side prints its multiaddr; the enter side must echo it
    // back via `--addr` (until mDNS-driven discovery lands in 5.2.c).
    // Confirming the helpful error rather than a generic clap error.
    let env = common::TestEnv::new();
    let cfg = tempfile::TempDir::new().unwrap();
    bypass(&env)
        .args(["sync", "pair", "--enter"])
        .env("XDG_CONFIG_HOME", cfg.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("--enter requires --addr"));
}

#[test]
fn sync_pair_with_neither_show_nor_enter_fails() {
    let env = common::TestEnv::new();
    let cfg = tempfile::TempDir::new().unwrap();
    bypass(&env)
        .args(["sync", "pair"])
        .env("XDG_CONFIG_HOME", cfg.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("--show"));
}

#[test]
fn edit_with_unchanged_buffer_reports_no_changes() {
    let env = common::TestEnv::new();
    bypass(&env)
        .arg("init")
        .arg(common::TEST_RECIPIENT)
        .assert()
        .success();
    bypass(&env)
        .args(["insert", "note"])
        .write_stdin("original")
        .assert()
        .success();

    // `true` is a no-op editor that doesn't touch the file.
    bypass(&env)
        .args(["edit", "note"])
        .env("EDITOR", "true")
        .assert()
        .success()
        .stderr(predicate::str::contains("no changes"));

    bypass(&env)
        .args(["show", "note"])
        .assert()
        .success()
        .stdout(predicate::str::starts_with("original"));
}
