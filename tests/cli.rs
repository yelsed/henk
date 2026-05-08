//! Integration tests for the `henk` binary.
//!
//! These exercise the public CLI surface using `assert_cmd`. They run the
//! actual binary, so they touch the real host (read-only — `init --dry-run`
//! never writes anything). They depend on `docker` not being mandatory: the
//! detection probes degrade gracefully when something is missing.

use assert_cmd::Command;
use predicates::prelude::*;

fn henk() -> Command {
    Command::cargo_bin("henk").expect("henk binary should exist")
}

#[test]
fn version_flag_prints_a_version() {
    henk()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::starts_with("henk "));
}

#[test]
fn help_lists_all_subcommands() {
    let assertion = henk().arg("--help").assert().success();
    let output = assertion.get_output();
    let stdout = String::from_utf8_lossy(&output.stdout);
    for cmd in [
        "init", "link", "unlink", "status", "up", "down", "doctor", "update", "uninstall",
    ] {
        assert!(
            stdout.contains(cmd),
            "expected --help output to mention `{cmd}`. Got:\n{stdout}"
        );
    }
}

#[test]
fn no_args_prints_smart_status_stub() {
    henk()
        .assert()
        .success()
        .stdout(predicate::str::contains("henk"))
        .stdout(predicate::str::contains("Run `henk init`"));
}

#[test]
fn init_dry_run_renders_detection_table() {
    let assertion = henk().args(["init", "--dry-run"]).assert().success();
    let output = assertion.get_output();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("environment detection"));
    assert!(stdout.contains("Docker"));
    assert!(stdout.contains("Homebrew"));
    assert!(stdout.contains("/etc/resolver/<tld>"));
    assert!(stdout.contains("TLD:"));
}

// `henk init` (full) is interactive and modifies the system. It's covered
// by the manual three-project verification scenario in
// docs/architecture.md, not by an automated CLI test.

#[test]
fn unimplemented_subcommands_bail() {
    // Removed as each milestone lands them: up/down (M2/M3), link (M4).
    for sub in ["status", "doctor", "update", "uninstall"] {
        henk()
            .arg(sub)
            .assert()
            .failure()
            .stderr(predicate::str::contains("not yet implemented"));
    }
}
