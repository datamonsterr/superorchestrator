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
        for hidden in &def.shadowed {
            let warning = format!(
                "agent `{}` at {} shadows {}",
                def.name,
                def.file.display(),
                hidden.display()
            );
            tracing::warn!(run_id = %row.id, "{warning}");
            self.emit(
                &row.session_id,
                Some(&row.id),
                so_core::EventType::Progress,
                serde_json::json!({"step": "agent discovery", "status": "warning", "note": warning}),
            ).await;
        }
        if !m.system_addendum_extra.trim().is_empty() {
            spec.system_addendum.push_str("\n\n");
            spec.system_addendum
                .push_str(m.system_addendum_extra.trim_end());
            spec.system_addendum.push('\n');
        }
        spec.agent = AgentLaunch {
            extra_args: checked_args(m.extra_args)?,
            config_fragment: m.config_fragment,
            agent_name: (harness == Harness::Opencode).then(|| def.name.clone()),
        };
        Ok(())
    }
}

/// Keeps `args` only if every flag is allowed and every value is not itself a flag. The run's
/// turn budget is on the row and reaches the harness through the driver, so the copy
/// materialisation put in Claude's argv is dropped (the call may have overridden it).
fn checked_args(args: Vec<String>) -> Result<Vec<String>, String> {
    so_agents::materialise::validate_claude_extra_args(&args)?;
    let mut out = Vec::with_capacity(args.len());
    let mut dropping = false;
    for a in args {
        if a.starts_with('-') {
            dropping = a == "--max-turns";
            if !dropping {
                out.push(a);
            }
        } else if out.is_empty() && !dropping {
            // A value with no flag before it.
            return Err(format!("the agent would add a stray argument `{a}`"));
        } else if !dropping {
            out.push(a);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn max_turns_pair_is_dropped_and_the_rest_kept() {
        assert_eq!(
            checked_args(v(&[
                "--tools",
                "Read",
                "--max-turns",
                "9",
                "--add-dir",
                "/x"
            ]))
            .unwrap(),
            v(&["--tools", "Read", "--add-dir", "/x"])
        );
        assert_eq!(
            checked_args(v(&["--tools", ""])).unwrap(),
            v(&["--tools", ""])
        );
    }

    #[test]
    fn other_flags_and_stray_values_are_refused() {
        for bad in [
            &["--dangerously-skip-permissions"][..],
            &["--tools", "Read", "--model", "x"],
            &["--tools", "-p"],
            &["Read"],
        ] {
            assert!(checked_args(v(bad)).is_err(), "{bad:?}");
        }
    }
}
