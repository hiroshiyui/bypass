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
