#![allow(dead_code, clippy::unwrap_used)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use assert_cmd::Command;

pub struct Env {
    pub tmp: tempfile::TempDir,
    pub cfg: PathBuf,
    pub home: PathBuf,
    pub bin: PathBuf,
}

impl Env {
    pub fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path().join("cfg");
        let home = tmp.path().join("home");
        let bin = tmp.path().join("bin");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        Self {
            tmp,
            cfg,
            home,
            bin,
        }
    }

    /// A `superorchestrator` command isolated from the real user environment.
    pub fn cmd(&self) -> Command {
        let mut c = Command::cargo_bin("superorchestrator").unwrap();
        c.arg("--config-dir")
            .arg(&self.cfg)
            .env("HOME", &self.home)
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()))
            .env("SUPERORCHESTRATOR_SOCKET", self.tmp.path().join("so.sock"))
            .env_remove("SUPERORCHESTRATOR_CONFIG_DIR")
            .env_remove("SUPERORCHESTRATOR_LOG")
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("EDITOR")
            .env_remove("VISUAL");
        c
    }

    pub fn script(&self, name: &str, body: &str) -> PathBuf {
        let p = self.bin.join(name);
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    pub fn init(&self) {
        self.cmd().arg("init").assert().success();
    }

    pub fn store(&self) -> so_store::Store {
        so_store::Store::open(self.cfg.join("state.db")).unwrap()
    }

    pub fn paths(&self) -> so_core::Paths {
        so_core::Paths::from_dir(self.cfg.clone())
    }
}

pub fn stdout(out: &assert_cmd::assert::Assert) -> String {
    String::from_utf8_lossy(&out.get_output().stdout).into_owned()
}

pub fn exists(p: &Path) -> bool {
    p.exists()
}

/// A binary of this workspace's `debug` profile, found next to the running test.
///
/// Test executables live in `<target>/debug/deps`, so the binary sits in the
/// grandparent directory. This works for any target dir, shared or per-checkout.
pub fn sibling_bin(name: &str) -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let debug = exe.parent().unwrap().parent().unwrap();
    debug.join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
}
