#![allow(clippy::unwrap_used)]

use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn version_prints_name_and_version() {
    Command::cargo_bin("superorchestrator")
        .unwrap()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("superorchestrator 0.1.0"));
}
