//! The starter agents `init` installs (SPEC 10).

use std::path::PathBuf;

use so_core::Paths;

use crate::error::{AgentError, Result};

const STARTERS: [(&str, &str); 3] = [
    ("ui-ux-designer", include_str!("starters/ui-ux-designer.md")),
    ("pr-reviewer", include_str!("starters/pr-reviewer.md")),
    (
        "issue-summarizer",
        include_str!("starters/issue-summarizer.md"),
    ),
];

pub fn starter_names() -> Vec<&'static str> {
    STARTERS.iter().map(|(n, _)| *n).collect()
}

/// Writes `<agents dir>/<name>/AGENT.md` for each starter that is not there yet,
/// in either folder or flat form, so a user's edit or removal-by-rename is
/// respected only while the file exists. Returns `(path, created)` per starter.
pub fn install_starters(paths: &Paths) -> Result<Vec<(PathBuf, bool)>> {
    let dir = paths.agents_dir();
    std::fs::create_dir_all(&dir).map_err(|e| AgentError::io(&dir, e))?;
    let mut out = Vec::new();
    for (name, text) in STARTERS {
        let folder = dir.join(name);
        let file = folder.join("AGENT.md");
        let flat = dir.join(format!("{name}.md"));
        if file.exists() || flat.exists() {
            out.push((file, false));
            continue;
        }
        std::fs::create_dir_all(&folder).map_err(|e| AgentError::io(&folder, e))?;
        std::fs::write(&file, text).map_err(|e| AgentError::io(&file, e))?;
        out.push((file, true));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::{AgentSource, parse_agent};

    #[test]
    fn starters_parse() {
        for (name, text) in STARTERS {
            let a = parse_agent(
                text,
                std::path::Path::new(name),
                None,
                AgentSource::User,
                name,
            )
            .unwrap();
            assert_eq!(a.name, name);
            assert!(a.prompt.len() > 100);
        }
    }

    #[test]
    fn install_is_idempotent_and_keeps_edits() {
        let t = tempfile::tempdir().unwrap();
        let paths = Paths::from_dir(t.path().join("cfg"));
        let first = install_starters(&paths).unwrap();
        assert!(first.iter().all(|(_, c)| *c) && first.len() == 3);
        let p = paths.agents_dir().join("pr-reviewer/AGENT.md");
        std::fs::write(&p, "edited").unwrap();
        let second = install_starters(&paths).unwrap();
        assert!(second.iter().all(|(_, c)| !*c));
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "edited");
    }
}
