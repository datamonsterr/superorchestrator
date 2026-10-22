//! Account files, credential homes, wrappers and login flows.
#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod env;
pub mod error;
pub mod host;
pub mod manage;
pub mod scan;
pub mod signin;
pub mod wrapper;

pub use env::{Launch, LaunchMode, build_launch, resolve_binary};
pub use error::{AccountsError, Result};
pub use host::HostEnv;
pub use manage::{LoginPlan, SELF_EXE, add, import, login_command, remove, set_enabled};
pub use scan::{ImportCandidate, scan, scan_new};
pub use signin::{SignIn, sign_in_state};
pub use wrapper::{is_managed, remove_wrapper, write_wrapper};

#[cfg(test)]
pub(crate) mod testutil {
    use std::path::{Path, PathBuf};

    use crate::HostEnv;
    use so_core::Platform;

    pub fn host(home: &Path, platform: Platform) -> HostEnv {
        HostEnv {
            home: home.to_path_buf(),
            path_var: Some(home.join("bin").to_string_lossy().into_owned()),
            platform,
        }
    }

    /// Creates an executable stub `name` in `<home>/bin`.
    pub fn stub_bin(home: &Path, name: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let dir = home.join("bin");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }
}
