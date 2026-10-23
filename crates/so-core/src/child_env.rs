//! Environment boundary for harness processes (GHSA-rfwj-83wf-qr2g).

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
        // agy needs the real user's keyring (ADR-0003); on Linux its session
        // bus address selects the Secret Service that holds that login.
        || name == "DBUS_SESSION_BUS_ADDRESS"
}

/// True for a literal environment variable name (non-empty, no NUL or `=`).
pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && !name.contains(['\0', '='])
}

/// Resolves explicitly opted-in inherited variables without converting their values
/// to UTF-8. Apply these before explicit driver/account settings.
pub fn pass_vars(pass_env: &[String]) -> impl Iterator<Item = (OsString, OsString)> + '_ {
    std::env::vars_os().filter(move |(key, _)| pass_env.iter().any(|name| key == OsStr::new(name)))
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
    fn explicit_values_and_removals_win() {
        let mut command = Command::new("unused");
        command
            .env("HOME", "/account")
            .env("SO_DRIVER_VAR", "driver")
            .env_remove("PATH");
        isolate(&mut command, &[]);
        let vars: std::collections::HashMap<_, _> = command.get_envs().collect();
        assert_eq!(vars[OsStr::new("HOME")], Some(OsStr::new("/account")));
        assert_eq!(
            vars[OsStr::new("SO_DRIVER_VAR")],
            Some(OsStr::new("driver"))
        );
        // After env_clear(), an explicitly removed key may disappear entirely.
        assert!(vars.get(OsStr::new("PATH")).is_none_or(|v| v.is_none()));
    }
}
