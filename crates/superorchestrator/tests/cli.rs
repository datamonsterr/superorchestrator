#![allow(clippy::unwrap_used)]

use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn version_prints_name_and_version() {
    let expected = format!("superorchestrator {}", env!("CARGO_PKG_VERSION"));
    Command::cargo_bin("superorchestrator")
        .unwrap()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains(expected));
}

#[test]
fn help_prints_about_text() {
    Command::cargo_bin("superorchestrator")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Delegate coding work across harnesses and accounts",
        ));
}
