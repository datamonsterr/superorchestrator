//! The Claude Code plugin in `plugin/` stays in step with the binary.
#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use serde_json::{Value, json};

fn plugin_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../plugin")
}

fn codex_plugin_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../plugin-codex")
}

fn json_file(rel: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(plugin_dir().join(rel)).unwrap()).unwrap()
}

#[test]
fn manifest_matches_the_release_version() {
    let m = json_file(".claude-plugin/plugin.json");
    assert_eq!(m["name"], "superorchestrator");
    assert_eq!(m["version"], env!("CARGO_PKG_VERSION"));
    assert!(m["description"].as_str().is_some_and(|d| !d.is_empty()));
}

#[test]
fn mcp_config_starts_the_mcp_server() {
    assert_eq!(
        json_file(".mcp.json"),
        json!({"mcpServers": {"superorchestrator": {"command": "superorchestrator", "args": ["mcp"]}}})
    );
}

/// The shipped skills match their embedded templates. Set
/// `UPDATE_PLUGIN_SKILL=1` to rewrite them.
#[test]
fn skills_are_the_generated_plugin_skills() {
    let skills = [
        (
            "use-superorchestrator-mcp",
            so_agents::skill::render_plugin_mcp_skill(),
        ),
        (
            "superorchestrator-cli",
            so_agents::skill::render_plugin_cli_skill(),
        ),
        (
            "superorchestrator-config",
            so_agents::skill::render_plugin_config_skill(),
        ),
    ];
    for (name, want) in skills {
        for dir in [plugin_dir(), codex_plugin_dir()] {
            let path = dir.join(format!("skills/{name}/SKILL.md"));
            if std::env::var_os("UPDATE_PLUGIN_SKILL").is_some() {
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(&path, &want).unwrap();
            }
            let have = std::fs::read_to_string(&path).unwrap();
            assert_eq!(
                have, want,
                "plugin/skills/{name}/SKILL.md is stale; run with UPDATE_PLUGIN_SKILL=1"
            );
        }
    }
    assert!(!plugin_dir().join("skills/superorchestrator").exists());
}

#[test]
fn codex_plugin_and_marketplace_are_wired() {
    let codex_dir = codex_plugin_dir();
    let read_codex = |rel: &str| -> Value {
        serde_json::from_slice(&std::fs::read(codex_dir.join(rel)).unwrap()).unwrap()
    };
    let m = read_codex(".codex-plugin/plugin.json");
    assert_eq!(m["name"], "superorchestrator");
    assert_eq!(
        m["version"],
        json_file(".claude-plugin/plugin.json")["version"]
    );
    assert_eq!(m["skills"], "./skills/");
    let claude_mcp = json_file(".mcp.json");
    let codex_mcp = read_codex(".mcp.json");
    let claude = &claude_mcp["mcpServers"]["superorchestrator"];
    let codex = &codex_mcp["mcpServers"]["superorchestrator"];
    assert_eq!(codex["command"], claude["command"]);
    assert_eq!(codex["args"], claude["args"]);
    assert_eq!(codex["env_vars"], json!(so_core::CODEX_MCP_ENV_VARS));
    assert_eq!(
        codex["startup_timeout_sec"],
        so_core::CODEX_STARTUP_TIMEOUT_SECS
    );
    assert_eq!(
        codex["tool_timeout_sec"],
        so_core::MAX_WAIT.as_secs() + so_core::CODEX_TOOL_TIMEOUT_MARGIN_SECS
    );
    for field in ["description", "homepage", "repository", "license"] {
        assert!(m[field].as_str().is_some_and(|s| !s.is_empty()));
    }
    assert!(m["author"]["name"].is_string());
    assert!(m["keywords"].is_array());
    let ext = &m["extensions"]["com.openai"];
    for field in [
        "displayName",
        "shortDescription",
        "longDescription",
        "developerName",
        "category",
        "websiteURL",
    ] {
        assert!(ext["interface"][field].is_string(), "{field}");
    }
    assert!(ext["interface"]["capabilities"].is_array());
    assert_eq!(ext["interface"]["category"], "Developer Tools");
    assert!(ext["interface"]["defaultPrompt"].is_array());
    let root = plugin_dir().parent().unwrap().to_path_buf();
    let market: Value = serde_json::from_slice(
        &std::fs::read(root.join(".agents/plugins/marketplace.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(market["name"], "superorchestrator");
    assert!(market["interface"]["displayName"].is_string());
    let entries = market["plugins"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert_eq!(entry["name"], "superorchestrator");
    assert_eq!(
        entry["source"],
        json!({"source":"local", "path":"./plugin-codex"})
    );
    assert_eq!(
        entry["policy"],
        json!({"installation":"AVAILABLE", "authentication":"ON_INSTALL"})
    );
    assert_eq!(entry["category"], "Developer Tools");
    assert!(
        root.join(entry["source"]["path"].as_str().unwrap())
            .join(".codex-plugin/plugin.json")
            .is_file()
    );
    let hooks = read_codex("hooks/hooks.json");
    assert!(
        hooks
            .as_object()
            .unwrap()
            .keys()
            .all(|key| key == "hooks" || key == "description")
    );
    assert_eq!(hooks["hooks"].as_object().unwrap().len(), 2);
    for (event, command) in [("SessionStart", "session-start"), ("Stop", "stop")] {
        assert_eq!(hooks["hooks"][event].as_array().unwrap().len(), 1);
        assert_eq!(
            hooks["hooks"][event][0]["hooks"].as_array().unwrap().len(),
            1
        );
        let h = &hooks["hooks"][event][0]["hooks"][0];
        assert_eq!(h["type"], "command");
        assert_eq!(h["command"], format!("superorchestrator hook {command}"));
        assert!(
            std::process::Command::new(env!("CARGO_BIN_EXE_superorchestrator"))
                .args(["hook", command, "--help"])
                .output()
                .unwrap()
                .status
                .success()
        );
        assert!(h["timeout"].as_u64().is_some_and(|n| n > 0 && n <= 10));
        assert!(h["statusMessage"].is_string());
    }
}

#[test]
fn all_codex_plugin_json_files_parse_and_hook_keys_are_supported() {
    fn check(dir: &std::path::Path) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                check(&path);
            } else if path.extension().is_some_and(|ext| ext == "json") {
                let value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                if path.ends_with("hooks/hooks.json") {
                    assert!(
                        value
                            .as_object()
                            .unwrap()
                            .keys()
                            .all(|key| key == "hooks" || key == "description")
                    );
                }
            }
        }
    }
    check(&codex_plugin_dir());
}

#[test]
fn hook_stop_without_supervisor_is_silent() {
    let dir = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_superorchestrator"))
        .args(["hook", "stop"])
        .env_remove("SUPERORCHESTRATOR_SOCKET")
        .env("XDG_RUNTIME_DIR", dir.path())
        .env("SUPERORCHESTRATOR_CONFIG_DIR", dir.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
}

#[test]
fn hook_stop_returns_when_stdin_remains_open() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_superorchestrator"))
        .args(["hook", "stop"])
        .env_remove("SUPERORCHESTRATOR_SOCKET")
        .env("XDG_RUNTIME_DIR", dir.path())
        .env("SUPERORCHESTRATOR_CONFIG_DIR", dir.path())
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("hook stop did not exit with stdin open");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[cfg(unix)]
#[test]
fn hook_stop_returns_when_supervisor_does_not_reply() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("s.sock");
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    std::thread::spawn(move || {
        let (_stream, _) = listener.accept().unwrap();
        std::thread::sleep(std::time::Duration::from_secs(5));
    });
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_superorchestrator"))
        .args(["hook", "stop"])
        .env("SUPERORCHESTRATOR_SOCKET", &socket)
        .env("SUPERORCHESTRATOR_CONFIG_DIR", dir.path())
        .stdin(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(4);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("hook stop did not exit with an unresponsive supervisor");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}
