//! Custom agents at launch (SPEC 10): the agent file is read again for every attempt and
//! written into the run directory in the harness's native format, so a resumed or failed-over
//! run gets the form its harness needs.

use std::path::PathBuf;

use so_agents::{materialise, resolve_for_spawn};
use so_core::Harness;
use so_harness::{AgentLaunch, LaunchSpec};
use so_store::RunRow;

use super::RunManager;

/// Where the agent's files go, inside the run directory.
const AGENT_DIR: &str = "agent";

impl RunManager {
    /// Adds the agent's instructions, tool controls and extra config to `spec`. Model, effort
    /// and max turns are already on the run row (resolved at spawn in the order call, agent,
    /// config).
    pub(crate) async fn apply_agent(
        &self,
        row: &RunRow,
        name: &str,
        spec: &mut LaunchSpec,
    ) -> Result<(), String> {
        let cwd = PathBuf::from(row.cwd.clone().ok_or("run has no working directory")?);
        let (paths, name) = (self.inner.paths.clone(), name.to_string());
        let dir = spec.run_dir.join(AGENT_DIR);
        let harness = row.harness;
        let (def, m) = self
            .blocking(move || -> Result<_, String> {
                let def = resolve_for_spawn(&cwd, &paths, &name).map_err(|e| e.to_string())?;
                // Files are created exclusively, so an earlier attempt's are cleared first.
                match std::fs::remove_dir_all(&dir) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(format!("could not clear {}: {e}", dir.display())),
                }
                let m =
                    materialise(&def, harness, &dir, &cwd, &paths).map_err(|e| e.to_string())?;
                Ok((def, m))
            })
            .await
            .map_err(|e| e.message)??;
        if !m.system_addendum_extra.trim().is_empty() {
            spec.system_addendum.push_str("\n\n");
            spec.system_addendum
                .push_str(m.system_addendum_extra.trim_end());
            spec.system_addendum.push('\n');
        }
        spec.agent = AgentLaunch {
            extra_args: without_max_turns(m.extra_args),
            config_fragment: m.config_fragment,
            agent_name: (harness == Harness::Opencode).then(|| def.name.clone()),
        };
        Ok(())
    }
}

/// The run's turn budget is on the row and goes to the harness through the driver, so the
/// copy materialisation put in Claude's argv is dropped (the call may have overridden it).
fn without_max_turns(args: Vec<String>) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len());
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        if a == "--max-turns" {
            it.next();
        } else {
            out.push(a);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_turns_pair_is_dropped_and_the_rest_kept() {
        let args = ["--tools", "Read", "--max-turns", "9", "--add-dir", "/x"]
            .map(String::from)
            .to_vec();
        assert_eq!(
            without_max_turns(args),
            ["--tools", "Read", "--add-dir", "/x"].map(String::from)
        );
    }
}
