#![allow(dead_code, clippy::unwrap_used)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

use assert_cmd::Command;
use serde_json::{Value, json};

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

// ---------------------------------------------------------------- fake harness

pub const TRANSCRIPT: &str =
    include_str!("../../../so-harness/tests/fixtures/claude/transcript.jsonl");

pub fn fake_harness() -> PathBuf {
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(target_dir_of_this_test);
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let st = StdCommand::new(cargo)
        .args([
            "build",
            "-p",
            "so-harness",
            "--features",
            "test-support",
            "--bin",
            "fake-harness",
        ])
        .status()
        .unwrap();
    assert!(st.success());
    target.join("debug/fake-harness")
}

/// Script replaying the first recorded claude turn.
pub fn script() -> String {
    let mut steps = vec![json!({"expect_stdin_contains": "go"})];
    for l in TRANSCRIPT.lines() {
        let v: Value = serde_json::from_str(l).unwrap();
        if v["dir"] != "in" {
            continue;
        }
        let last = v["msg"]["type"] == "result";
        steps.push(json!({"stdout": v["msg"]}));
        if last {
            break;
        }
    }
    steps.iter().map(|s| format!("{s}\n")).collect()
}

pub fn add_account(e: &Env) {
    let script_path = e.tmp.path().join("script.jsonl");
    std::fs::write(&script_path, script()).unwrap();
    let wrapper = e.tmp.path().join("claude-acc1.sh");
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nexport FAKE_HARNESS_SCRIPT={}\nexec {} \"$@\"\n",
            script_path.display(),
            fake_harness().display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::create_dir_all(e.cfg.join("accounts")).unwrap();
    std::fs::write(
        e.cfg.join("accounts/claude-acc1.toml"),
        format!(
            "name = \"claude-acc1\"\nharness = \"claude\"\nhome = \"{}\"\nbinary = \"{}\"\n",
            e.home.join(".claude-acc1").display(),
            wrapper.display()
        ),
    )
    .unwrap();
}

pub async fn cli(e: &Env, args: &[&str]) -> (String, bool) {
    let mut cmd = e.cmd();
    cmd.args(args);
    tokio::task::spawn_blocking(move || {
        let out = cmd.output().unwrap();
        if !out.status.success() {
            // Without this a failed call only shows up as a bare `assert!(ok)`.
            eprintln!(
                "cli {:?} failed ({}):\nstdout: {}\nstderr: {}",
                cmd.get_args().collect::<Vec<_>>(),
                out.status,
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        }
        (
            String::from_utf8_lossy(&out.stdout).into_owned(),
            out.status.success(),
        )
    })
    .await
    .unwrap()
}

/// `<target>/debug/deps/<this test>` gives the target directory when `CARGO_TARGET_DIR` is
/// not set (a `build.target-dir` in cargo's config is invisible to the test).
fn target_dir_of_this_test() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| Some(p.parent()?.parent()?.parent()?.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target"))
}

/// An in-process supervisor for the CLI to talk to (it finds the socket through
/// `SUPERORCHESTRATOR_SOCKET`, which `Env::cmd` sets).
pub async fn start_supervisor(e: &Env) -> so_supervisor::SupervisorHandle {
    // The socket directory must be private.
    std::fs::set_permissions(e.tmp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let host = so_accounts::HostEnv {
        home: e.home.clone(),
        path_var: Some("/usr/bin:/bin".into()),
        platform: so_core::Platform::current(),
    };
    let opts = so_supervisor::SupervisorOptions::new(
        e.paths(),
        so_core::Config::default(),
        e.tmp.path().join("so.sock"),
    )
    .with_host(host)
    .with_idle_exit(std::time::Duration::from_secs(600))
    .with_setup(so_supervisor::default_handlers);
    so_supervisor::Supervisor::start(opts).await.unwrap()
}
