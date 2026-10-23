//! Environment boundary for processes the supervisor daemon spawns (GHSA-rfwj-83wf-qr2g).
//! Interactive CLI commands keep the operator's own environment.

use std::ffi::{OsStr, OsString};
use std::process::Command;

/// Whether an inherited variable belongs to the baseline harness environment.
fn allowed(name: &OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    matches!(name,
        "PATH" | "HOME" | "USER" | "LOGNAME" | "SHELL" | "LANG" |
        "TERM" | "COLORTERM" | "TZ" | "TMPDIR" | "SSL_CERT_FILE" | "SSL_CERT_DIR"
    ) || name.starts_with("LC_") || name.starts_with("XDG_")
        || ["HTTP_PROXY", "HTTPS_PROXY", "NO_PROXY", "ALL_PROXY"]
            .iter().any(|proxy| name.eq_ignore_ascii_case(proxy))
        // Needed for the agy keyring (ADR-0003): on Linux the session bus
        // address selects the Secret Service that holds the login. This also
        // exposes the operator's session bus to every harness.
        || name == "DBUS_SESSION_BUS_ADDRESS"
}

/// True for a literal environment variable name (non-empty, no NUL or `=`).
pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && !name.contains(['\0', '='])
}

/// Joins inherited-name lists in order, dropping duplicates, so account and global
/// `pass_env` entries form one effective list.
pub fn merge_pass_env<'a>(lists: impl IntoIterator<Item = &'a [String]>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for name in lists.into_iter().flatten() {
        if !out.contains(name) {
            out.push(name.clone());
        }
    }
    out
}

/// Starts from an empty environment, inherits only baseline and explicitly named
/// variables, then restores caller-set values and removals. Values stay as OS
/// strings, and explicit driver/account settings always win over inheritance.
pub fn isolate(command: &mut Command, pass_env: &[String]) {
    let explicit: Vec<(OsString, Option<OsString>)> = command
        .get_envs()
        .map(|(k, v)| (k.to_owned(), v.map(OsStr::to_owned)))
        .collect();
    command.env_clear();
    command.envs(
        std::env::vars_os()
            .filter(|(k, _)| allowed(k) || pass_env.iter().any(|name| k == OsStr::new(name))),
    );
    for (key, value) in explicit {
        match value {
            Some(value) => {
                command.env(key, value);
            }
            None => {
                command.env_remove(key);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baseline_is_allowlist_not_secret_denylist() {
        for name in [
            "PATH",
            "HOME",
            "LC_ALL",
            "LC_MESSAGES",
            "XDG_RUNTIME_DIR",
            "http_proxy",
            "hTtPs_PrOxY",
            "NO_PROXY",
            "ALL_PROXY",
            "SSL_CERT_FILE",
            "DBUS_SESSION_BUS_ADDRESS",
        ] {
            assert!(allowed(OsStr::new(name)), "{name}");
        }
        for name in [
            "SO_TEST_SECRET",
            "GITHUB_TOKEN",
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "OPENROUTER_API_KEY",
            "AWS_ACCESS_KEY_ID",
            "OPENCODE_SERVER_PASSWORD",
            "CODEX_HOME",
            "CLAUDE_CONFIG_DIR",
            "DISPLAY",
            "WAYLAND_DISPLAY",
            "SSH_AUTH_SOCK",
            "NODE_OPTIONS",
            "LD_PRELOAD",
        ] {
            assert!(!allowed(OsStr::new(name)), "{name}");
        }
    }

    #[test]
    fn explicit_values_win_and_merge_dedups() {
        let mut command = Command::new("unused");
        command
            .env("HOME", "/account")
            .env("SO_DRIVER_VAR", "driver");
        isolate(&mut command, &[]);
        let vars: std::collections::HashMap<_, _> = command.get_envs().collect();
        assert_eq!(vars[OsStr::new("HOME")], Some(OsStr::new("/account")));
        assert_eq!(
            vars[OsStr::new("SO_DRIVER_VAR")],
            Some(OsStr::new("driver"))
        );
        let a = vec!["A".to_string(), "B".to_string()];
        let b = vec!["B".to_string(), "C".to_string()];
        assert_eq!(
            merge_pass_env([a.as_slice(), b.as_slice()]),
            ["A", "B", "C"]
        );
    }

    #[test]
    fn explicit_removal_survives_allowlist_reinstatement() {
        // PATH is on the allowlist, so isolate() re-adds it; the removal must be replayed after.
        let mut command = Command::new("/usr/bin/env");
        command.env_remove("PATH");
        isolate(&mut command, &[]);
        let out = command.output().unwrap();
        assert!(out.status.success());
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(!text.lines().any(|l| l.starts_with("PATH=")), "{text}");
    }
}
