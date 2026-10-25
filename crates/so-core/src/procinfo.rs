//! Process liveness and identity, used to tell whether a recorded wrapper session is
//! still running (a pid alone can be reused by an unrelated process).

use std::collections::HashMap;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

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
/// process. Linux reads `starttime` from `/proc/<pid>/stat`; other systems read
/// `ps -o lstart=` under a fixed locale and time zone and keep it as epoch seconds, so
/// the token does not depend on the environment of the caller. `None` when it cannot be
/// read.
pub fn process_start(pid: i64) -> Option<String> {
    let token = if cfg!(target_os = "linux") {
        std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .and_then(|stat| parse_stat_starttime(&stat))
    } else {
        ps_start(pid)
    };
    if token.is_none() {
        tracing::debug!(pid, "could not read the process start time");
    }
    token
}

#[cfg_attr(target_os = "linux", allow(dead_code))]
fn ps_start(pid: i64) -> Option<String> {
    let out = Command::new("ps")
        .args(["-o", "lstart=", "-p", &pid.to_string()])
        .env("LC_ALL", "C")
        .env("TZ", "UTC")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_lstart(&String::from_utf8_lossy(&out.stdout))
}

/// `Wed Oct 22 10:11:12 2026` (UTC, C locale) as epoch seconds.
#[cfg_attr(target_os = "linux", allow(dead_code))]
fn parse_lstart(s: &str) -> Option<String> {
    let t = chrono::NaiveDateTime::parse_from_str(s.trim(), "%a %b %e %H:%M:%S %Y").ok()?;
    Some(t.and_utc().timestamp().to_string())
}

/// [`process_start`], remembered for [`START_TTL`] where reading it spawns `ps`.
fn process_start_cached(pid: i64) -> Option<String> {
    if cfg!(target_os = "linux") {
        return process_start(pid);
    }
    type Seen = HashMap<i64, (Option<String>, Instant)>;
    static CACHE: Mutex<Option<Seen>> = Mutex::new(None);
    let mut g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let map = g.get_or_insert_with(HashMap::new);
    map.retain(|_, (_, at)| at.elapsed() < START_TTL);
    if let Some((tok, _)) = map.get(&pid) {
        return tok.clone();
    }
    drop(g);
    let tok = process_start(pid);
    let mut g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    g.get_or_insert_with(HashMap::new)
        .insert(pid, (tok.clone(), Instant::now()));
    tok
}

const START_TTL: Duration = Duration::from_secs(30);

/// Field 22 of `/proc/<pid>/stat`. The command name (field 2) may contain spaces and
/// parentheses, so count from the last `)`.
fn parse_stat_starttime(stat: &str) -> Option<String> {
    let rest = stat.get(stat.rfind(')')? + 1..)?;
    // `rest` starts at field 3, so field 22 is index 19.
    rest.split_whitespace().nth(19).map(str::to_string)
}

/// The parent pid of `pid`. On Linux it reads field 4 (ppid) of `/proc/<pid>/stat`, counting
/// from the last `)` so a command name with spaces or parentheses is safe. Other systems run
/// `ps -o ppid= -p <pid>` under a fixed locale. `None` when it cannot be read.
pub fn parent_pid(pid: i64) -> Option<i64> {
    if cfg!(target_os = "linux") {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        parse_stat_ppid(&stat)
    } else {
        ps_ppid(pid)
    }
}

#[cfg_attr(target_os = "linux", allow(dead_code))]
fn ps_ppid(pid: i64) -> Option<i64> {
    let out = Command::new("ps")
        .args(["-o", "ppid=", "-p", &pid.to_string()])
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// Field 4 of `/proc/<pid>/stat`. The command name (field 2) may contain spaces and
/// parentheses, so count from the last `)`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_stat_ppid(stat: &str) -> Option<i64> {
    let rest = stat.get(stat.rfind(')')? + 1..)?;
    // `rest` starts at field 3 (state), so field 4 is index 1.
    rest.split_whitespace().nth(1)?.parse().ok()
}

/// True when the process recorded as `(pid, start)` is still the one running under that
/// pid. Without a recorded or readable start token, liveness alone decides.
pub fn same_process_alive(pid: i64, recorded_start: Option<&str>) -> bool {
    if !pid_alive(pid) {
        return false;
    }
    match (recorded_start, process_start_cached(pid)) {
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
    fn parent_pid_of_own_process_equals_the_real_parent() {
        let me = i64::from(std::process::id());
        let ppid = parent_pid(me).expect("own parent is readable");
        assert!(ppid > 0, "parent pid should be positive, got {ppid}");
        // Independent of the implementation: ask the system again.
        let out = Command::new("ps")
            .args(["-o", "ppid=", "-p", &me.to_string()])
            .output()
            .unwrap();
        let real: i64 = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap();
        assert_eq!(ppid, real);
    }

    #[test]
    fn parent_pid_of_a_missing_process_is_none() {
        assert!(parent_pid(i64::from(i32::MAX)).is_none());
        assert!(parent_pid(-1).is_none());
        assert!(parent_pid(0).is_none());
    }

    #[test]
    fn ppid_parsing_survives_odd_command_names() {
        let stat = "123 (a) b (c) S 4242 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15";
        assert_eq!(parse_stat_ppid(stat), Some(4242));
        assert_eq!(parse_stat_ppid("not a stat line"), None);
    }

    #[test]
    fn lstart_parses_to_epoch_seconds_with_padded_days() {
        assert_eq!(
            parse_lstart("Thu Jan  1 00:00:10 1970\n").as_deref(),
            Some("10")
        );
        assert_eq!(
            parse_lstart(" Thu Oct 22 10:11:12 2026 ").as_deref(),
            Some("1792663872")
        );
        assert_eq!(
            parse_lstart("Sun Oct  4 10:11:12 2026").as_deref(),
            Some("1791108672")
        );
        assert!(parse_lstart("").is_none());
        assert!(parse_lstart("not a date").is_none());
    }

    #[test]
    fn stat_parsing_survives_odd_command_names() {
        let stat = "123 (a) b (c) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 4242 23 24";
        assert_eq!(parse_stat_starttime(stat).as_deref(), Some("4242"));
    }
}
