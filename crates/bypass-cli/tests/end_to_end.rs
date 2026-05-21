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
