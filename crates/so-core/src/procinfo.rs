//! Process liveness and identity, used to tell whether a recorded wrapper session is
//! still running (a pid alone can be reused by an unrelated process).

use std::process::{Command, Stdio};

/// True when a process with this pid exists (signal 0; EPERM still means it exists).
pub fn pid_alive(pid: i64) -> bool {
    let Some(p) = i32::try_from(pid)
        .ok()
        .filter(|p| *p > 0)
        .and_then(rustix::process::Pid::from_raw)
    else {
        return false;
    };
    match rustix::process::test_kill_process(p) {
        Ok(()) => true,
        Err(e) => e != rustix::io::Errno::SRCH,
    }
}

/// An opaque token for when the process started; equal tokens for one pid mean the same
/// process. Linux reads `starttime` from `/proc/<pid>/stat`; other systems use
/// `ps -o lstart=`. `None` when it cannot be read.
pub fn process_start(pid: i64) -> Option<String> {
    if cfg!(target_os = "linux") {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        return parse_stat_starttime(&stat);
    }
    let out = Command::new("ps")
        .args(["-o", "lstart=", "-p", &pid.to_string()])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !s.is_empty()).then_some(s)
}

/// Field 22 of `/proc/<pid>/stat`. The command name (field 2) may contain spaces and
/// parentheses, so count from the last `)`.
fn parse_stat_starttime(stat: &str) -> Option<String> {
    let rest = stat.get(stat.rfind(')')? + 1..)?;
    // `rest` starts at field 3, so field 22 is index 19.
    rest.split_whitespace().nth(19).map(str::to_string)
}

/// True when the process recorded as `(pid, start)` is still the one running under that
/// pid. Without a recorded or readable start token, liveness alone decides.
pub fn same_process_alive(pid: i64, recorded_start: Option<&str>) -> bool {
    if !pid_alive(pid) {
        return false;
    }
    match (recorded_start, process_start(pid)) {
        (Some(was), Some(now)) => was == now,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_process_is_alive_with_a_stable_start() {
        let me = i64::from(std::process::id());
        assert!(pid_alive(me));
        let a = process_start(me);
        assert!(a.is_some());
        assert_eq!(a, process_start(me));
        assert!(same_process_alive(me, a.as_deref()));
        assert!(same_process_alive(me, None));
    }

    #[test]
    fn missing_pid_is_dead() {
        assert!(!pid_alive(i64::from(i32::MAX)));
        assert!(!pid_alive(-1));
        assert!(!pid_alive(0));
        assert!(!same_process_alive(i64::from(i32::MAX), None));
    }

    #[test]
    fn a_reused_pid_with_another_start_is_not_the_same_process() {
        let me = i64::from(std::process::id());
        assert!(!same_process_alive(me, Some("1")));
    }

    #[test]
    fn stat_parsing_survives_odd_command_names() {
        let stat = "123 (a) b (c) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 4242 23 24";
        assert_eq!(parse_stat_starttime(stat).as_deref(), Some("4242"));
    }
}
