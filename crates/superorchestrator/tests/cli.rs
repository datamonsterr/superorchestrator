#![allow(clippy::unwrap_used)]

mod common;

use common::Env;
use predicates::prelude::*;

#[test]
fn version_prints_name_and_version() {
    let expected = format!("superorchestrator {}", env!("CARGO_PKG_VERSION"));
    let e = Env::new();
    e.cmd()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains(expected));
}

#[test]
fn help_prints_about_text() {
    let e = Env::new();
    e.cmd()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Delegate coding work across harnesses and accounts",
        ));
}
