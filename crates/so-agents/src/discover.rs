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
    let mut out = Discovery::default();
    for (dir, source) in search_dirs(cwd, paths) {
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
                Ok(def) => match out.agents.iter().find(|a| a.name == def.name) {
                    Some(winner) => out.shadowed.push((def.file, winner.file.clone())),
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

/// Deletes an agent's folder (or its flat file). The caller checks `source.read_only()`.
pub fn remove_files(def: &AgentDef) -> Result<()> {
    match &def.dir {
        Some(dir) => std::fs::remove_dir_all(dir).map_err(|e| AgentError::io(dir, e)),
        None => std::fs::remove_file(&def.file).map_err(|e| AgentError::io(&def.file, e)),
    }
}

/// Finds the agent a spawn asked for. Entry point for the supervisor.
pub fn resolve_for_spawn(cwd: &Path, paths: &Paths, name: &str) -> Result<AgentDef> {
    let d = discover(cwd, paths);
    if let Some(a) = d.agents.into_iter().find(|a| a.name == name) {
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
    fn empty_when_dirs_missing() {
        let t = tempfile::tempdir().unwrap();
        let d = discover(t.path(), &Paths::from_dir(t.path().join("none")));
        assert!(d.agents.is_empty() && d.problems.is_empty());
    }
}
