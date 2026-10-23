//! Agent discovery (SPEC 10). Search order, earlier wins on a name clash:
//! 1. `<cwd>/.superorchestrator/agents/`
//! 2. `<config dir>/agents/`
//! 3. `<cwd>/.claude/agents/` (read-only import)
//!
//! Each directory holds `<name>/AGENT.md` folders and flat `<name>.md` files.

use std::path::{Path, PathBuf};

use so_core::Paths;

use crate::error::{AgentError, Result};
use crate::parse::{AgentDef, AgentSource, parse_agent};

/// Result of a discovery pass. Unreadable or invalid files are reported, not fatal.
#[derive(Debug, Default)]
pub struct Discovery {
    pub agents: Vec<AgentDef>,
    pub problems: Vec<(PathBuf, String)>,
    /// Same-named agents hidden by an earlier location: `(hidden file, winner file)`.
    pub shadowed: Vec<(PathBuf, PathBuf)>,
}

/// The three search directories in order.
pub fn search_dirs(cwd: &Path, paths: &Paths) -> [(PathBuf, AgentSource); 3] {
    [
        (
            cwd.join(".superorchestrator/agents"),
            AgentSource::Superorchestrator,
        ),
        (paths.agents_dir(), AgentSource::User),
        (cwd.join(".claude/agents"), AgentSource::ClaudeImport),
    ]
}

pub fn discover(cwd: &Path, paths: &Paths) -> Discovery {
    discover_in(search_dirs(cwd, paths))
}

/// Only the agents in the config dir: those that do not depend on a working directory.
pub fn discover_user(paths: &Paths) -> Discovery {
    discover_in([(paths.agents_dir(), AgentSource::User)])
}

fn discover_in(dirs: impl IntoIterator<Item = (PathBuf, AgentSource)>) -> Discovery {
    let mut out = Discovery::default();
    for (dir, source) in dirs {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut entries: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
        entries.sort();
        for p in entries {
            let (file, folder, stem) = if p.is_dir() {
                let stem = file_name(&p);
                (p.join("AGENT.md"), Some(p.clone()), stem)
            } else if p.extension().is_some_and(|e| e == "md") {
                let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned());
                (p.clone(), None, stem.unwrap_or_default())
            } else {
                continue;
            };
            if !file.is_file() {
                continue;
            }
            match read_one(&file, folder, source, &stem) {
                Ok(def) => match out.agents.iter_mut().find(|a| a.name == def.name) {
                    Some(winner) => {
                        winner.shadowed.push(def.file.clone());
                        out.shadowed.push((def.file, winner.file.clone()));
                    }
                    None => out.agents.push(def),
                },
                Err(e) => out.problems.push((file, e.to_string())),
            }
        }
    }
    out
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn read_one(
    file: &Path,
    dir: Option<PathBuf>,
    source: AgentSource,
    stem: &str,
) -> Result<AgentDef> {
    let (text, cut) = crate::parse::read_capped(file, crate::parse::MAX_AGENT_FILE)
        .map_err(|e| AgentError::io(file, e))?;
    if cut {
        return Err(AgentError::Parse {
            path: file.to_path_buf(),
            msg: format!("larger than {} KiB", crate::parse::MAX_AGENT_FILE / 1024),
        });
    }
    parse_agent(&text, file, dir, source, stem)
}

fn problem_name(file: &Path) -> String {
    let is_folder_agent = file.file_name().is_some_and(|n| n == "AGENT.md");
    let part = if is_folder_agent {
        file.parent().and_then(Path::file_name)
    } else {
        file.file_stem()
    };
    part.map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

impl AgentDef {
    /// The raw text of the agent file (at most `MAX_AGENT_FILE`).
    pub fn read_source(&self) -> Result<String> {
        read_source_of(&self.file)
    }
}

/// The raw text of an agent file, whether or not it parses.
pub fn read_source_of(file: &Path) -> Result<String> {
    let (text, cut) = crate::parse::read_capped(file, crate::parse::MAX_AGENT_FILE)
        .map_err(|e| AgentError::io(file, e))?;
    if cut {
        return Err(AgentError::Parse {
            path: file.to_path_buf(),
            msg: format!("larger than {} KiB", crate::parse::MAX_AGENT_FILE / 1024),
        });
    }
    Ok(text)
}

/// Deletes an agent's folder (or its flat file). The caller checks `source.read_only()`.
pub fn remove_files(def: &AgentDef) -> Result<()> {
    match &def.dir {
        Some(dir) => std::fs::remove_dir_all(dir).map_err(|e| AgentError::io(dir, e)),
        None => std::fs::remove_file(&def.file).map_err(|e| AgentError::io(&def.file, e)),
    }
}

/// Finds an agent for inspection, including definitions that cannot be spawned.
pub fn resolve(cwd: &Path, paths: &Paths, name: &str) -> Result<AgentDef> {
    let d = discover(cwd, paths);
    if let Some(a) = d.agents.into_iter().find(|a| a.name == name) {
        // Like the listings: a symlinked agent may point outside the agents directories.
        let link = |p: &Path| std::fs::symlink_metadata(p).is_ok_and(|m| m.is_symlink());
        if link(&a.file) || a.dir.as_deref().is_some_and(link) {
            return Err(AgentError::Parse {
                path: a.file,
                msg: "symlinked agents cannot be spawned".into(),
            });
        }
        return Ok(a);
    }
    // A broken file named like the request: report why it failed, not "not found".
    // Its folder name or file stem is the only name we have.
    if let Some((path, msg)) = d
        .problems
        .into_iter()
        .find(|(p, _)| problem_name(p) == name)
    {
        return Err(AgentError::Parse { path, msg });
    }
    Err(AgentError::NotFound(name.to_string()))
}

/// Resolves and validates an agent before the supervisor selects isolation or launches it.
/// Repeated on every launch attempt, including resume and failover. The actual harness is
/// not known during resolution; Claude argument checks happen after selection or at
/// materialisation, since the definition's preference can be overridden.
pub fn resolve_for_spawn(cwd: &Path, paths: &Paths, name: &str) -> Result<AgentDef> {
    let def = resolve(cwd, paths, name)?;
    crate::materialise::validate_for_spawn(&def, paths, None)?;
    Ok(def)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent_md(name: &str, desc: &str) -> String {
        format!("---\nname: {name}\ndescription: {desc}\n---\nbody of {desc}\n")
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn trust_refusal_hints_only_mention_a_project_copy_that_precedes_user_agents() {
        let t = tempfile::tempdir().unwrap();
        let cwd = t.path().join("proj");
        let paths = Paths::from_dir(t.path().join("cfg"));
        for (location, project_hint) in [
            (".claude/agents", false),
            (".superorchestrator/agents", true),
        ] {
            let file = cwd.join(location).join("unsafe.md");
            write(
                &file,
                "---\nname: unsafe\ndescription: d\nhooks: {}\n---\nbody",
            );
            let err = resolve_for_spawn(&cwd, &paths, "unsafe")
                .unwrap_err()
                .to_string();
            assert_eq!(err.contains("remove any same-named"), project_hint, "{err}");
            assert!(err.contains(paths.agents_dir().to_str().unwrap()), "{err}");
            std::fs::remove_file(file).unwrap();
        }
    }

    #[test]
    fn project_command_fields_are_refused_at_spawn_but_remain_discoverable() {
        let t = tempfile::tempdir().unwrap();
        let paths = Paths::from_dir(t.path().join("cfg"));
        let cwd = t.path().join("proj");
        let mut accepted = Vec::new();
        for location in [".superorchestrator/agents", ".claude/agents"] {
            for (field, yaml) in [
                ("hooks", "hooks: {}"),
                (
                    "hooks",
                    "hooks: {PreToolUse: [{hooks: [{type: command, command: evil}]}]}",
                ),
                ("mcpServers", "mcpServers: [{evil: {command: evil}}]"),
                (
                    "mcpServers",
                    "mcpServers: [{evil: {command: [sh, -c, evil]}}]",
                ),
                ("mcpServers", "mcpServers: [{evil: {args: [sh, -c, evil]}}]"),
                (
                    "mcpServers",
                    "mcpServers: [{evil: {url: https://example.test, command: evil}}]",
                ),
                ("mcpServers", "mcpServers: [{evil: [sh, -c, evil]}]"),
                ("isolation", "isolation: none"),
                ("extra_args", "extra_args: []"),
                ("extra_args", "extra_args: [--tools, Read]"),
                ("extra_args", "extraArgs: [--tools, Read]"),
            ] {
                let file = cwd.join(location).join("hostile.md");
                write(
                    &file,
                    &format!("---\nname: hostile\ndescription: d\n{yaml}\n---\nbody"),
                );
                assert!(
                    discover(&cwd, &paths)
                        .agents
                        .iter()
                        .any(|a| a.name == "hostile")
                );
                let err = match resolve_for_spawn(&cwd, &paths, "hostile") {
                    Err(e) => e.to_string(),
                    Ok(_) => {
                        accepted.push(format!("{location}: {yaml}"));
                        std::fs::remove_file(file).unwrap();
                        continue;
                    }
                };
                for expected in [
                    "hostile",
                    file.to_str().unwrap(),
                    field,
                    paths.agents_dir().to_str().unwrap(),
                ] {
                    assert!(err.contains(expected), "{yaml}: missing {expected}: {err}");
                }
                std::fs::remove_file(file).unwrap();
            }
        }
        assert!(accepted.is_empty(), "unsafe agents accepted: {accepted:?}");
    }

    #[test]
    fn project_agents_can_use_named_and_remote_mcp_servers() {
        let t = tempfile::tempdir().unwrap();
        let paths = Paths::from_dir(t.path().join("cfg"));
        let cwd = t.path().join("proj");
        for yaml in [
            "mcpServers: [docs]",
            "mcpServers: [{docs: {type: http, url: https://example.test, headers: {Authorization: token}}}]",
            "mcpServers: [{docs: {type: sse, url: https://example.test}}]",
        ] {
            write(
                &cwd.join(".superorchestrator/agents/helper.md"),
                &format!("---\nname: helper\ndescription: d\n{yaml}\n---\nbody"),
            );
            assert!(resolve_for_spawn(&cwd, &paths, "helper").is_ok(), "{yaml}");
        }
    }

    #[test]
    fn a_refusal_names_all_offending_fields_and_trusted_user_agents_still_spawn() {
        let t = tempfile::tempdir().unwrap();
        let paths = Paths::from_dir(t.path().join("cfg"));
        let cwd = t.path().join("proj");
        let text = "---\nname: hostile\ndescription: d\nhooks: {Stop: []}\nmcpServers: [{evil: {command: evil}}]\nisolation: none\nextra_args: [--tools, Read]\n---\nbody";
        write(&cwd.join(".superorchestrator/agents/hostile.md"), text);
        let err = resolve_for_spawn(&cwd, &paths, "hostile")
            .unwrap_err()
            .to_string();
        for field in ["hooks", "mcpServers", "isolation", "extra_args"] {
            assert!(err.contains(field), "{err}");
        }
        std::fs::remove_dir_all(cwd.join(".superorchestrator")).unwrap();
        write(&paths.agents_dir().join("hostile.md"), text);
        assert!(resolve_for_spawn(&cwd, &paths, "hostile").is_ok());
        write(
            &paths.agents_dir().join("hostile.md"),
            "---\nname: hostile\ndescription: d\nextra_args: [--settings-unknown, evil]\n---\nbody",
        );
        // Resolution has not selected a harness, so Claude argument validation waits.
        assert!(resolve_for_spawn(&cwd, &paths, "hostile").is_ok());
    }

    #[test]
    fn order_clash_and_sources() {
        let t = tempfile::tempdir().unwrap();
        let cwd = t.path().join("proj");
        let paths = Paths::from_dir(t.path().join("cfg"));
        write(
            &cwd.join(".superorchestrator/agents/dup/AGENT.md"),
            &agent_md("dup", "project"),
        );
        write(&paths.agents_dir().join("dup.md"), &agent_md("dup", "user"));
        write(
            &cwd.join(".claude/agents/dup.md"),
            &agent_md("dup", "claude"),
        );
        write(
            &paths.agents_dir().join("only-user/AGENT.md"),
            &agent_md("only-user", "u"),
        );
        write(&cwd.join(".claude/agents/imp.md"), &agent_md("imp", "i"));
        write(&cwd.join(".claude/agents/notes.txt"), "ignored");
        write(&cwd.join(".claude/agents/broken.md"), "no frontmatter");

        let d = discover(&cwd, &paths);
        let by = |n: &str| d.agents.iter().find(|a| a.name == n).unwrap();
        assert_eq!(d.agents.len(), 3);
        assert_eq!(by("dup").description, "project");
        assert_eq!(by("dup").source, AgentSource::Superorchestrator);
        assert!(by("dup").dir.is_some());
        assert_eq!(by("only-user").source, AgentSource::User);
        assert_eq!(by("imp").source, AgentSource::ClaudeImport);
        assert!(by("imp").dir.is_none());
        assert_eq!(d.shadowed.len(), 2);
        assert_eq!(d.problems.len(), 1);
        assert!(d.problems[0].0.ends_with("broken.md"));
    }

    #[test]
    fn resolve_and_not_found() {
        let t = tempfile::tempdir().unwrap();
        let paths = Paths::from_dir(t.path().join("cfg"));
        write(&paths.agents_dir().join("a.md"), &agent_md("a", "x"));
        assert_eq!(resolve_for_spawn(t.path(), &paths, "a").unwrap().name, "a");
        assert!(matches!(
            resolve_for_spawn(t.path(), &paths, "zzz"),
            Err(AgentError::NotFound(_))
        ));
        write(
            &paths.agents_dir().join("broken/AGENT.md"),
            "no frontmatter",
        );
        write(
            &paths.agents_dir().join("flat-broken.md"),
            "---\nname: x\n---\n",
        );
        for n in ["broken", "flat-broken"] {
            match resolve_for_spawn(t.path(), &paths, n) {
                Err(AgentError::Parse { .. }) => {}
                other => panic!("{n}: {other:?}"),
            }
        }
    }

    #[test]
    fn a_symlinked_agent_is_not_spawnable() {
        let t = tempfile::tempdir().unwrap();
        let paths = Paths::from_dir(t.path().join("cfg"));
        write(&t.path().join("elsewhere.md"), &agent_md("linked", "x"));
        std::fs::create_dir_all(paths.agents_dir()).unwrap();
        std::os::unix::fs::symlink(
            t.path().join("elsewhere.md"),
            paths.agents_dir().join("linked.md"),
        )
        .unwrap();
        assert!(matches!(
            resolve_for_spawn(t.path(), &paths, "linked"),
            Err(AgentError::Parse { .. })
        ));
    }

    #[test]
    fn empty_when_dirs_missing() {
        let t = tempfile::tempdir().unwrap();
        let d = discover(t.path(), &Paths::from_dir(t.path().join("none")));
        assert!(d.agents.is_empty() && d.problems.is_empty());
    }
}
