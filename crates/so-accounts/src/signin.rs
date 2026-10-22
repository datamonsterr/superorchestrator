//! Sign-in check by auth-file existence only. Contents are never read.

use so_core::{AccountFile, Harness, Platform};

use crate::env::claude_default_home;
use crate::error::Result;
use crate::host::{HostEnv, same_path};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignIn {
    SignedIn,
    SignedOut,
    /// Cannot tell, e.g. Claude credentials may live in the OS keyring.
    Unknown,
}

/// Auth files per harness, relative paths under the credential home.
/// Verified on a real machine: codex `auth.json`, claude `.credentials.json`,
/// opencode `auth.json` (default install) or `data/opencode/auth.json`
/// (isolated home), agy `.gemini/antigravity-cli/antigravity-oauth-token`
/// (plus the older `.gemini/oauth_creds.json` and `google_accounts.json`).
pub fn sign_in_state(account: &AccountFile, host: &HostEnv) -> Result<SignIn> {
    let home = host.expand(&account.home)?;
    let present = |rels: &[&str]| rels.iter().any(|r| home.join(r).exists());
    Ok(match account.harness {
        Harness::Codex => bool_state(present(&["auth.json"])),
        Harness::Claude => {
            if present(&[".credentials.json"]) {
                SignIn::SignedIn
            } else if host.platform == Platform::MacOs
                || same_path(&home, &claude_default_home(host))
            {
                SignIn::Unknown
            } else {
                SignIn::SignedOut
            }
        }
        Harness::Opencode => bool_state(present(&["auth.json", "data/opencode/auth.json"])),
        Harness::Agy => bool_state(present(&[
            ".gemini/antigravity-cli/antigravity-oauth-token",
            ".gemini/oauth_creds.json",
            ".gemini/google_accounts.json",
        ])),
    })
}

fn bool_state(b: bool) -> SignIn {
    if b {
        SignIn::SignedIn
    } else {
        SignIn::SignedOut
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::host;
    use std::os::unix::fs::PermissionsExt;

    fn acct(h: Harness, home: &str) -> AccountFile {
        AccountFile::new("test-acc", h, home, h.as_str()).unwrap()
    }

    fn touch_unreadable(p: &std::path::Path) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, [0xff, 0xfe, 0x00]).unwrap();
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o000)).unwrap();
    }

    #[test]
    fn existence_only_even_when_unreadable() {
        let d = tempfile::tempdir().unwrap();
        let h = host(d.path(), Platform::Linux);
        let r = d.path();
        let cases = [
            (Harness::Codex, "~/.c", ".c/auth.json"),
            (Harness::Claude, "~/.cl", ".cl/.credentials.json"),
            (Harness::Opencode, "~/.oc", ".oc/data/opencode/auth.json"),
            (
                Harness::Opencode,
                "~/.local/share/opencode",
                ".local/share/opencode/auth.json",
            ),
            (
                Harness::Agy,
                "~/.a",
                ".a/.gemini/antigravity-cli/antigravity-oauth-token",
            ),
            (Harness::Agy, "~/.a2", ".a2/.gemini/oauth_creds.json"),
            (Harness::Agy, "~/.a3", ".a3/.gemini/google_accounts.json"),
        ];
        for (harness, home, rel) in cases {
            let a = acct(harness, home);
            assert_eq!(sign_in_state(&a, &h).unwrap(), SignIn::SignedOut, "{rel}");
            touch_unreadable(&r.join(rel));
            assert_eq!(sign_in_state(&a, &h).unwrap(), SignIn::SignedIn, "{rel}");
        }
    }

    #[test]
    fn claude_missing_file_is_unknown_on_macos() {
        let d = tempfile::tempdir().unwrap();
        let h = host(d.path(), Platform::MacOs);
        assert_eq!(
            sign_in_state(&acct(Harness::Claude, "~/.claude-two"), &h).unwrap(),
            SignIn::Unknown
        );
    }

    #[test]
    fn claude_default_home_without_file_is_unknown() {
        let d = tempfile::tempdir().unwrap();
        let h = host(d.path(), Platform::Linux);
        assert_eq!(
            sign_in_state(&acct(Harness::Claude, "~/.claude"), &h).unwrap(),
            SignIn::Unknown
        );
        assert_eq!(
            sign_in_state(&acct(Harness::Claude, "~/.claude-two"), &h).unwrap(),
            SignIn::SignedOut
        );
    }
}
