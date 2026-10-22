//! Pure edits of the harness config files that register (or unregister) our MCP server.
//! Each function takes the current file text and returns the new text, or `None` when
//! nothing needs to change. Only the `superorchestrator` key is ever touched.

use anyhow::{Context, Result, anyhow};
use serde_json::{Map, Value, json};
use toml_edit::{Array, DocumentMut, Item, Table, value};

/// Key of our entry in every harness config.
pub const NAME: &str = "superorchestrator";

/// Codex `config.toml`: `[mcp_servers.superorchestrator]` with `command` and `args`.
pub fn codex_add(old: &str, exe: &str) -> Result<Option<String>> {
    let mut doc: DocumentMut = old.parse().context("not valid TOML")?;
    let servers = doc
        .entry("mcp_servers")
        .or_insert_with(|| {
            let mut t = Table::new();
            t.set_implicit(true);
            Item::Table(t)
        })
        .as_table_mut()
        .ok_or_else(|| anyhow!("`mcp_servers` is not a table"))?;
    let entry = servers
        .entry(NAME)
        .or_insert_with(|| Item::Table(Table::new()))
        .as_table_mut()
        .ok_or_else(|| anyhow!("`mcp_servers.{NAME}` is not a table"))?;
    let same_command = entry.get("command").and_then(Item::as_str) == Some(exe);
    let same_args = entry
        .get("args")
        .and_then(Item::as_array)
        .is_some_and(|a| a.len() == 1 && a.get(0).and_then(|v| v.as_str()) == Some("mcp"));
    if same_command && same_args {
        return Ok(None);
    }
    entry["command"] = value(exe);
    let mut args = Array::new();
    args.push("mcp");
    entry["args"] = value(args);
    Ok(Some(doc.to_string()))
}

pub fn codex_remove(old: &str) -> Result<Option<String>> {
    codex_remove_key(old, NAME)
}

/// Removes `[mcp_servers.<key>]`, leaving every other server alone.
pub fn codex_remove_key(old: &str, key: &str) -> Result<Option<String>> {
    let mut doc: DocumentMut = old.parse().context("not valid TOML")?;
    let Some(servers) = doc.get_mut("mcp_servers").and_then(Item::as_table_mut) else {
        return Ok(None);
    };
    if servers.remove(key).is_none() {
        return Ok(None);
    }
    Ok(Some(doc.to_string()))
}

/// Which JSON config shape to write.
#[derive(Debug, Clone, Copy)]
pub enum JsonKind {
    /// `opencode.json`: `mcp.superorchestrator = { type, command: [exe, "mcp"], enabled }`.
    Opencode,
    /// agy `config/mcp_config.json`: `mcpServers.superorchestrator = { command, args, disabled }`.
    Agy,
}

impl JsonKind {
    fn parent(self) -> &'static str {
        match self {
            Self::Opencode => "mcp",
            Self::Agy => "mcpServers",
        }
    }

    fn entry(self, exe: &str) -> Value {
        match self {
            Self::Opencode => json!({ "type": "local", "command": [exe, "mcp"], "enabled": true }),
            Self::Agy => json!({ "command": exe, "args": ["mcp"], "disabled": false }),
        }
    }
}

fn parse_object(old: &str) -> Result<Map<String, Value>> {
    if old.trim().is_empty() {
        return Ok(Map::new());
    }
    match serde_json::from_str(old).context("not valid JSON")? {
        Value::Object(m) => Ok(m),
        _ => Err(anyhow!("top level is not a JSON object")),
    }
}

fn render(root: Map<String, Value>) -> Result<String> {
    let mut s = serde_json::to_string_pretty(&Value::Object(root))?;
    s.push('\n');
    Ok(s)
}

pub fn json_add(old: &str, kind: JsonKind, exe: &str) -> Result<Option<String>> {
    let mut root = parse_object(old)?;
    let parent = root
        .entry(kind.parent())
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| anyhow!("`{}` is not an object", kind.parent()))?;
    let want = kind.entry(exe);
    match parent.get_mut(NAME) {
        // Keep keys the user added to our entry; only the ones we own must match.
        Some(Value::Object(have)) => {
            let Value::Object(want) = want else {
                unreachable!("entries are objects")
            };
            let mut changed = false;
            for (k, v) in want {
                if have.get(&k) != Some(&v) {
                    have.insert(k, v);
                    changed = true;
                }
            }
            if !changed {
                return Ok(None);
            }
        }
        _ => {
            parent.insert(NAME.to_string(), want);
        }
    }
    render(root).map(Some)
}

pub fn json_remove(old: &str, kind: JsonKind) -> Result<Option<String>> {
    json_remove_key(old, kind, NAME)
}

/// Removes the server entry `key`, leaving every other server alone.
pub fn json_remove_key(old: &str, kind: JsonKind, key: &str) -> Result<Option<String>> {
    let mut root = parse_object(old)?;
    let Some(parent) = root.get_mut(kind.parent()).and_then(Value::as_object_mut) else {
        return Ok(None);
    };
    if parent.shift_remove(key).is_none() {
        return Ok(None);
    }
    if parent.is_empty() {
        root.shift_remove(kind.parent());
    }
    render(root).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXE: &str = "/opt/so \"x\"/superorchestrator";

    #[test]
    fn codex_add_preserves_comments_and_other_servers_and_is_idempotent() {
        let old = "# my config\nmodel = \"x\"\n\n[mcp_servers.other]\ncommand = \"o\" # keep\n";
        let new = codex_add(old, EXE).unwrap().unwrap();
        assert!(new.contains("# my config"));
        assert!(new.contains("command = \"o\" # keep"));
        assert!(new.contains("[mcp_servers.superorchestrator]"));
        let doc: DocumentMut = new.parse().unwrap();
        assert_eq!(
            doc["mcp_servers"]["superorchestrator"]["command"].as_str(),
            Some(EXE)
        );
        assert_eq!(codex_add(&new, EXE).unwrap(), None);
        let back = codex_remove(&new).unwrap().unwrap();
        assert!(back.contains("[mcp_servers.other]"));
        assert!(!back.contains("superorchestrator"));
        assert_eq!(codex_remove(&back).unwrap(), None);
    }

    #[test]
    fn codex_roundtrip_on_empty_file_leaves_no_residue() {
        let new = codex_add("", EXE).unwrap().unwrap();
        assert_eq!(codex_remove(&new).unwrap().unwrap().trim(), "");
    }

    #[test]
    fn codex_updates_a_stale_path_and_keeps_extra_keys() {
        let old = "[mcp_servers.superorchestrator]\ncommand = \"/old\"\nargs = [\"mcp\"]\nenv = { A = \"1\" }\n";
        let new = codex_add(old, "/new").unwrap().unwrap();
        assert!(new.contains("command = \"/new\""));
        assert!(new.contains("env = { A = \"1\" }"));
    }

    #[test]
    fn json_add_remove_opencode_preserves_other_keys() {
        let old = r#"{"model":"m","mcp":{"ctx":{"type":"remote","url":"u"}}}"#;
        let new = json_add(old, JsonKind::Opencode, "/x/so").unwrap().unwrap();
        let v: Value = serde_json::from_str(&new).unwrap();
        assert_eq!(v["model"], "m");
        assert_eq!(v["mcp"]["ctx"]["url"], "u");
        assert_eq!(
            v["mcp"][NAME],
            json!({"type":"local","command":["/x/so","mcp"],"enabled":true})
        );
        // Key order of existing keys is kept.
        assert!(new.find("\"model\"") < new.find("\"mcp\""));
        assert_eq!(json_add(&new, JsonKind::Opencode, "/x/so").unwrap(), None);
        let back: Value =
            serde_json::from_str(&json_remove(&new, JsonKind::Opencode).unwrap().unwrap()).unwrap();
        assert!(back["mcp"].get(NAME).is_none());
        assert_eq!(back["mcp"]["ctx"]["url"], "u");
    }

    #[test]
    fn json_agy_creates_and_removes_empty_parent() {
        let new = json_add("", JsonKind::Agy, "/x/so").unwrap().unwrap();
        let v: Value = serde_json::from_str(&new).unwrap();
        assert_eq!(v["mcpServers"][NAME]["args"], json!(["mcp"]));
        let back = json_remove(&new, JsonKind::Agy).unwrap().unwrap();
        assert_eq!(serde_json::from_str::<Value>(&back).unwrap(), json!({}));
    }

    #[test]
    fn invalid_files_are_errors_not_overwritten() {
        assert!(json_add("{ nope", JsonKind::Agy, "/x").is_err());
        assert!(codex_add("= bad", "/x").is_err());
        assert!(json_add("[1]", JsonKind::Agy, "/x").is_err());
    }
}
