//! The Claude Code plugin in `plugin/` stays in step with the binary.
#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use serde_json::{Value, json};

fn plugin_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../plugin")
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
        let path = plugin_dir().join(format!("skills/{name}/SKILL.md"));
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
    assert!(!plugin_dir().join("skills/superorchestrator").exists());
}
