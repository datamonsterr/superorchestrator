//! The system addendum every run gets (SPEC 7.4).

use serde_json::Value;

/// Highest depth a run may have; fixed by SPEC 7.7.
pub const MAX_DEPTH: u8 = 2;

pub fn child_instructions(
    run_id: &str,
    depth: u8,
    parent: Option<&str>,
    output_schema: Option<&Value>,
) -> String {
    let parent = parent.map_or("the orchestrator".to_string(), |p| format!("run {p}"));
    let mut s = format!(
        "You are a subagent run of superorchestrator.\n\
         - Your run id is {run_id}. Your depth is {depth}. Your parent is {parent}.\n\
         - Never wait for a person. Nobody is watching this session.\n\
         - Use the ask_parent tool for decisions and blockers.\n\
         - Call report_progress(step, status) at each step of your work.\n\
         - Finish with a final result.\n"
    );
    if let Some(schema) = output_schema {
        s.push_str(&format!(
            "- Your final result must be a single JSON value that matches this JSON Schema, \
             with no other text:\n{schema}\n"
        ));
    }
    if depth < MAX_DEPTH {
        s.push_str("- You may call spawn_agent to delegate parts of your task.\n");
    } else {
        s.push_str(&format!(
            "- You are at the maximum depth ({MAX_DEPTH}); do not call spawn_agent.\n"
        ));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn covers_the_spec_points() {
        let s = child_instructions(
            "r1",
            1,
            Some("p0"),
            Some(&serde_json::json!({"type": "object"})),
        );
        for want in [
            "r1",
            "depth is 1",
            "run p0",
            "Never wait for a person",
            "ask_parent",
            "report_progress(step, status)",
            "JSON Schema",
            "may call spawn_agent",
        ] {
            assert!(s.contains(want), "missing {want}: {s}");
        }
        let deep = child_instructions("r2", 2, None, None);
        assert!(deep.contains("do not call spawn_agent") && !deep.contains("JSON Schema"));
    }
}
