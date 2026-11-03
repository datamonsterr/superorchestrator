use so_core::ErrorKind;
use so_core::Harness;

#[derive(Debug)]
pub struct AcpSpec {
    pub launch: &'static [&'static str],
    pub model_flag: Option<&'static str>,
    pub effort_flag: Option<&'static str>,
    pub join_flag: &'static str,
    pub error_patterns: &'static [(&'static str, ErrorKind)],
}

pub fn spec(harness: Harness) -> Option<AcpSpec> {
    let s = match harness {
        Harness::Copilot => AcpSpec {
            launch: &["--acp", "--allow-all-tools", "--allow-all-paths"],
            model_flag: Some("--model"),
            effort_flag: Some("--reasoning-effort"),
            join_flag: "--resume=",
            error_patterns: &[],
        },
        Harness::Gemini => AcpSpec {
            launch: &["--acp", "--approval-mode", "yolo"],
            model_flag: Some("-m"),
            effort_flag: None,
            join_flag: "--resume",
            error_patterns: &[],
        },
        Harness::Kimi => AcpSpec {
            launch: &["acp"],
            model_flag: None,
            effort_flag: None,
            join_flag: "--session",
            error_patterns: &[],
        },
        // Cursor is not installed here; global flags are documented but not verified in ACP mode.
        Harness::Cursor => AcpSpec {
            launch: &["acp", "--force", "--approve-mcps"],
            model_flag: Some("--model"),
            effort_flag: None,
            join_flag: "--resume",
            error_patterns: &[],
        },
        Harness::Qwen => AcpSpec {
            launch: &["--acp", "--approval-mode", "yolo"],
            model_flag: Some("-m"),
            effort_flag: None,
            join_flag: "--resume",
            error_patterns: &[],
        },
        _ => return None,
    };
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rows_have_expected_launch_and_join() {
        for (h, launch, model, effort, join) in [
            (
                Harness::Copilot,
                &(["--acp", "--allow-all-tools", "--allow-all-paths"][..]),
                Some("--model"),
                Some("--reasoning-effort"),
                "--resume=",
            ),
            (
                Harness::Gemini,
                &(["--acp", "--approval-mode", "yolo"][..]),
                Some("-m"),
                None,
                "--resume",
            ),
            (Harness::Kimi, &(["acp"][..]), None, None, "--session"),
            (
                Harness::Cursor,
                &(["acp", "--force", "--approve-mcps"][..]),
                Some("--model"),
                None,
                "--resume",
            ),
            (
                Harness::Qwen,
                &(["--acp", "--approval-mode", "yolo"][..]),
                Some("-m"),
                None,
                "--resume",
            ),
        ] {
            let s = spec(h).unwrap();
            assert_eq!(
                (s.launch, s.model_flag, s.effort_flag, s.join_flag),
                (launch, model, effort, join)
            );
        }
    }
}
