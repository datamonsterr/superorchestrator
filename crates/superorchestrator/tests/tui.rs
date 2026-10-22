#![allow(clippy::unwrap_used)]

mod common;

use common::{Env, stdout};
use predicates::prelude::*;

#[test]
fn bare_command_without_a_terminal_prints_help() {
    let e = Env::new();
    // Not `e.cmd()`: that adds `--config-dir`, and the bare form is what is under test.
    let mut c = assert_cmd::Command::cargo_bin("superorchestrator").unwrap();
    c.env("HOME", &e.home)
        .env("SUPERORCHESTRATOR_CONFIG_DIR", &e.cfg)
        .env("SUPERORCHESTRATOR_SOCKET", e.tmp.path().join("so.sock"));
    common::scrub(&mut c);
    let out = c.assert().success();
    let text = stdout(&out);
    assert!(
        text.contains("Usage:") && text.contains("Commands:"),
        "{text}"
    );
    assert!(text.contains("tui"), "the tui command is listed:\n{text}");
    // Help only: no escape sequences, nothing created.
    assert!(!text.contains('\x1b'));
    assert!(!e.cfg.exists());
}

#[test]
fn tui_without_a_terminal_is_a_usage_error() {
    let e = Env::new();
    e.cmd()
        .arg("tui")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("needs a terminal"))
        .stdout(predicate::str::is_empty());
    assert!(!e.cfg.exists());
}
