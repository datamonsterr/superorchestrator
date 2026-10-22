//! Glossary types shared by every crate: harnesses, runs, choices, headroom, events.

use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::CoreError;

/// Implements `ALL`, `as_str`, `Display` and `FromStr` for a unit enum, so the
/// wire name of each variant is written once. Tests assert it equals the serde name.
macro_rules! str_enum {
    (@unit $v:ident) => {
        ()
    };
    ($t:ident, $err:expr, { $($v:ident => $s:expr),+ $(,)? }) => {
        impl $t {
            pub const ALL: [$t; <[()]>::len(&[$(str_enum!(@unit $v)),+])] = [$($t::$v),+];

            pub fn as_str(&self) -> &'static str {
                match self {
                    $($t::$v => $s),+
                }
            }
        }

        impl fmt::Display for $t {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl FromStr for $t {
            type Err = CoreError;
            fn from_str(s: &str) -> ::core::result::Result<Self, Self::Err> {
                $t::ALL
                    .into_iter()
                    .find(|v| v.as_str() == s)
                    .ok_or_else(|| ($err)(s.to_string()))
            }
        }
    };
}

/// Generates a time-ordered unique id (UUID v7).
pub fn new_id() -> String {
    uuid::Uuid::now_v7().to_string()
}

/// A coding-agent CLI family.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum Harness {
    Codex,
    Agy,
    Opencode,
    Claude,
}

str_enum!(Harness, CoreError::UnknownHarness, {
    Codex => "codex",
    Agy => "agy",
    Opencode => "opencode",
    Claude => "claude",
});

/// The router-level effort hint for a model that reasons by itself (its model id is the
/// thinking variant). No harness takes it as a flag value: `agy` drivers launch without
/// `--effort` (ADR-0004), and the router ranks it with the top effort tier.
pub const EFFORT_THINKING: &str = "thinking";

impl Harness {
    /// Efforts the harness CLI accepts as a flag or config value, from its help on this
    /// machine (ADR-0004). `None` means the value is provider-specific and not validated
    /// (opencode `--variant`).
    pub fn accepted_efforts(self) -> Option<&'static [&'static str]> {
        match self {
            Harness::Claude => Some(&["low", "medium", "high", "xhigh", "max"]),
            // `thinking` is the hint above, dropped at launch.
            Harness::Agy => Some(&["low", "medium", "high", "xhigh", "max", EFFORT_THINKING]),
            Harness::Codex => Some(&["minimal", "low", "medium", "high", "xhigh", "max", "ultra"]),
            Harness::Opencode => None,
        }
    }

    /// Why `effort` cannot be used with this harness, or `None` when it can.
    pub fn effort_problem(self, effort: &str) -> Option<String> {
        let ok = self.accepted_efforts()?;
        if ok.contains(&effort) {
            return None;
        }
        let list = ok
            .iter()
            .filter(|e| **e != EFFORT_THINKING)
            .copied()
            .collect::<Vec<_>>()
            .join("|");
        Some(format!(
            "effort `{effort}` is not valid for {self} ({list})"
        ))
    }

    /// The effort to pass to the harness CLI: the router hint is dropped.
    pub fn launch_effort(self, effort: Option<&str>) -> Option<&str> {
        effort.filter(|e| *e != EFFORT_THINKING)
    }
}

impl Choice {
    /// Why this choice cannot launch because of its effort, or `None`.
    pub fn effort_problem(&self) -> Option<String> {
        self.harness.effort_problem(self.effort.as_deref()?)
    }
}

/// Lifecycle state of a run (SPEC 7.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Queued,
    Starting,
    Running,
    Waiting,
    Stalled,
    Escalated,
    PendingReview,
    Succeeded,
    Failed,
    Cancelled,
}

impl RunState {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            RunState::Succeeded | RunState::Failed | RunState::Cancelled
        )
    }

    pub fn is_active(&self) -> bool {
        !self.is_terminal()
    }

    /// Whether the transition `self -> to` is allowed by SPEC 7.1.
    pub fn can_transition(&self, to: RunState) -> bool {
        use RunState::*;
        if self.is_terminal() {
            return false;
        }
        if matches!(to, Failed | Cancelled) {
            return true;
        }
        matches!(
            (*self, to),
            (Queued, Starting)
                | (Starting, Running)
                | (Running, Waiting)
                | (Waiting, Running)
                | (Running, Stalled)
                | (Stalled, Running)
                | (Stalled, Escalated)
                | (Escalated, Running)
                | (Running, PendingReview)
                | (PendingReview, Succeeded)
                | (PendingReview, Running)
                | (Running, Succeeded)
        )
    }
}

str_enum!(RunState, |s| CoreError::UnknownValue { kind: "run state", value: s }, {
    Queued => "queued",
    Starting => "starting",
    Running => "running",
    Waiting => "waiting",
    Stalled => "stalled",
    Escalated => "escalated",
    PendingReview => "pending_review",
    Succeeded => "succeeded",
    Failed => "failed",
    Cancelled => "cancelled",
});

/// What kind of connection a session is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionKind {
    Orchestrator,
    Interactive,
}

str_enum!(SessionKind, |s| CoreError::UnknownValue { kind: "session kind", value: s }, {
    Orchestrator => "orchestrator",
    Interactive => "interactive",
});

fn default_roles() -> Vec<String> {
    vec!["worker".to_string()]
}

fn default_true() -> bool {
    true
}

/// One routable combination of harness, model and effort (SPEC 8.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Choice {
    pub id: String,
    pub harness: Harness,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default = "default_roles")]
    pub roles: Vec<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// Health of an (account, pool) headroom record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HeadroomStatus {
    Ok,
    Low,
    Exhausted,
    Unknown,
}

str_enum!(HeadroomStatus, |s| CoreError::UnknownValue { kind: "headroom status", value: s }, {
    Ok => "ok",
    Low => "low",
    Exhausted => "exhausted",
    Unknown => "unknown",
});

/// Where a headroom record came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HeadroomSource {
    Probe,
    Event,
    Ledger,
}

str_enum!(HeadroomSource, |s| CoreError::UnknownValue { kind: "headroom source", value: s }, {
    Probe => "probe",
    Event => "event",
    Ledger => "ledger",
});

/// Headroom of one (account, pool) (SPEC 6.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Headroom {
    pub account: String,
    pub pool: String,
    #[serde(default)]
    pub five_hour_remaining_pct: Option<f64>,
    #[serde(default)]
    pub five_hour_resets_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub weekly_remaining_pct: Option<f64>,
    #[serde(default)]
    pub weekly_resets_at: Option<DateTime<Utc>>,
    pub status: HeadroomStatus,
    pub source: HeadroomSource,
    /// When the numbers were measured. The Unix epoch is the internal "never observed" sentinel
    /// (always stale); on the wire that is `null`.
    #[serde(with = "observed_at_wire")]
    #[schemars(with = "Option<DateTime<Utc>>")]
    pub observed_at: DateTime<Utc>,
}

/// `observed_at` as JSON: the epoch sentinel is `null`, so clients never show 1970.
mod observed_at_wire {
    use chrono::{DateTime, Utc};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(t: &DateTime<Utc>, s: S) -> Result<S::Ok, S::Error> {
        (*t != DateTime::<Utc>::UNIX_EPOCH)
            .then_some(t)
            .serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<DateTime<Utc>, D::Error> {
        Ok(Option::<DateTime<Utc>>::deserialize(d)?.unwrap_or(DateTime::UNIX_EPOCH))
    }
}

/// Classification of a failed run's error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    Quota,
    RateLimit,
    Auth,
    Other,
}

str_enum!(ErrorKind, |s| CoreError::UnknownValue { kind: "error kind", value: s }, {
    Quota => "quota",
    RateLimit => "rate_limit",
    Auth => "auth",
    Other => "other",
});

/// Where a run executes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Isolation {
    Worktree,
    None,
}

str_enum!(Isolation, |s| CoreError::UnknownValue { kind: "isolation", value: s }, {
    Worktree => "worktree",
    None => "none",
});

/// Kind of a message between runs and the orchestrator (SPEC 9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    Message,
    Progress,
    Question,
    Answer,
    Nudge,
    Result,
}

str_enum!(MessageKind, |s| CoreError::UnknownValue { kind: "message kind", value: s }, {
    Message => "message",
    Progress => "progress",
    Question => "question",
    Answer => "answer",
    Nudge => "nudge",
    Result => "result",
});

/// Every event type of SPEC 12.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    RunQueued,
    RunStarted,
    StateChanged,
    RunFinished,
    AttemptStarted,
    Failover,
    RouteDecision,
    ToolCallStarted,
    ToolCallFinished,
    AssistantMessage,
    Progress,
    MessageSent,
    QuestionAsked,
    QuestionAnswered,
    Usage,
    UserJoined,
    UserLeft,
    JoinCommandIssued,
    StallDetected,
    NudgeSent,
    Escalated,
    WorktreeCreated,
    WorktreeRemoved,
    CheckResult,
    ReviewAccepted,
    ReviewRejected,
}

str_enum!(EventType, |s| CoreError::UnknownValue { kind: "event type", value: s }, {
    RunQueued => "run_queued",
    RunStarted => "run_started",
    StateChanged => "state_changed",
    RunFinished => "run_finished",
    AttemptStarted => "attempt_started",
    Failover => "failover",
    RouteDecision => "route_decision",
    ToolCallStarted => "tool_call_started",
    ToolCallFinished => "tool_call_finished",
    AssistantMessage => "assistant_message",
    Progress => "progress",
    MessageSent => "message_sent",
    QuestionAsked => "question_asked",
    QuestionAnswered => "question_answered",
    Usage => "usage",
    UserJoined => "user_joined",
    UserLeft => "user_left",
    JoinCommandIssued => "join_command_issued",
    StallDetected => "stall_detected",
    NudgeSent => "nudge_sent",
    Escalated => "escalated",
    WorktreeCreated => "worktree_created",
    WorktreeRemoved => "worktree_removed",
    CheckResult => "check_result",
    ReviewAccepted => "review_accepted",
    ReviewRejected => "review_rejected",
});

/// One appended fact in the timeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Event {
    pub id: String,
    pub session_id: String,
    #[serde(default)]
    pub run_id: Option<String>,
    pub ts: DateTime<Utc>,
    #[serde(rename = "type")]
    pub event_type: EventType,
    #[serde(default)]
    pub data: serde_json::Value,
}

#[cfg(test)]
mod tests {
    #[test]
    fn effort_validation_per_harness() {
        assert!(Harness::Agy.effort_problem("thinking").is_none());
        assert!(Harness::Agy.effort_problem("max").is_none());
        assert_eq!(
            Harness::Claude.effort_problem("thinking").unwrap(),
            "effort `thinking` is not valid for claude (low|medium|high|xhigh|max)"
        );
        assert_eq!(
            Harness::Agy.effort_problem("bogus").unwrap(),
            "effort `bogus` is not valid for agy (low|medium|high|xhigh|max)"
        );
        assert!(Harness::Codex.effort_problem("minimal").is_none());
        assert!(Harness::Codex.effort_problem("thinking").is_some());
        assert!(Harness::Opencode.effort_problem("anything").is_none());
        assert_eq!(Harness::Agy.launch_effort(Some("thinking")), None);
        assert_eq!(Harness::Agy.launch_effort(Some("high")), Some("high"));
    }

    use super::*;
    use RunState::*;

    #[test]
    fn harness_from_str_display_all() {
        for h in Harness::ALL {
            assert_eq!(h.to_string().parse::<Harness>().unwrap(), h);
        }
        assert_eq!("agy".parse::<Harness>().unwrap(), Harness::Agy);
        assert!("gemini".parse::<Harness>().is_err());
        assert_eq!(
            serde_json::to_string(&Harness::Opencode).unwrap(),
            "\"opencode\""
        );
    }

    #[test]
    fn state_serde_snake_case() {
        assert_eq!(
            serde_json::to_string(&PendingReview).unwrap(),
            "\"pending_review\""
        );
        assert_eq!(
            serde_json::from_str::<RunState>("\"escalated\"").unwrap(),
            Escalated
        );
    }

    #[test]
    fn active_and_terminal() {
        for s in [
            Queued,
            Starting,
            Running,
            Waiting,
            Stalled,
            Escalated,
            PendingReview,
        ] {
            assert!(s.is_active() && !s.is_terminal(), "{s}");
        }
        for s in [Succeeded, Failed, Cancelled] {
            assert!(s.is_terminal() && !s.is_active(), "{s}");
        }
    }

    #[test]
    fn allowed_transitions() {
        let ok = [
            (Queued, Starting),
            (Starting, Running),
            (Running, Waiting),
            (Waiting, Running),
            (Running, Stalled),
            (Stalled, Running),
            (Stalled, Escalated),
            (Escalated, Running),
            (Running, PendingReview),
            (PendingReview, Succeeded),
            (PendingReview, Running),
            (Running, Succeeded),
            (Starting, Failed),
            (Queued, Cancelled),
            (Escalated, Failed),
            (PendingReview, Cancelled),
        ];
        for (a, b) in ok {
            assert!(a.can_transition(b), "{a} -> {b}");
        }
    }

    #[test]
    fn forbidden_transitions() {
        let bad = [
            (Queued, Running),
            (Starting, Succeeded),
            (Waiting, Succeeded),
            (Stalled, Succeeded),
            (Running, Queued),
            (Running, Escalated),
            (Queued, PendingReview),
            (Succeeded, Running),
            (Failed, Running),
            (Cancelled, Failed),
            (Succeeded, Failed),
            (Running, Running),
        ];
        for (a, b) in bad {
            assert!(!a.can_transition(b), "{a} -> {b}");
        }
    }

    fn assert_str_matches_serde<T>(all: &[T])
    where
        T: Serialize + std::str::FromStr + fmt::Display + PartialEq + fmt::Debug,
        T::Err: fmt::Debug,
    {
        for v in all {
            let json = serde_json::to_value(v).unwrap();
            assert_eq!(json.as_str().unwrap(), v.to_string());
            assert_eq!(&v.to_string().parse::<T>().unwrap(), v);
        }
        assert!("no-such-value".parse::<T>().is_err());
    }

    #[test]
    fn as_str_matches_serde_for_every_enum() {
        assert_str_matches_serde(&Harness::ALL);
        assert_str_matches_serde(&RunState::ALL);
        assert_str_matches_serde(&SessionKind::ALL);
        assert_str_matches_serde(&HeadroomStatus::ALL);
        assert_str_matches_serde(&HeadroomSource::ALL);
        assert_str_matches_serde(&ErrorKind::ALL);
        assert_str_matches_serde(&Isolation::ALL);
        assert_str_matches_serde(&MessageKind::ALL);
        assert_str_matches_serde(&EventType::ALL);
    }

    #[test]
    fn choice_rejects_unknown_fields() {
        assert!(
            toml::from_str::<Choice>("id = \"a\"\nharness = \"codex\"\nmodel = \"m\"\nbogus = 1")
                .is_err()
        );
    }

    #[test]
    fn choice_defaults() {
        let c: Choice = toml::from_str("id = \"a\"\nharness = \"codex\"\nmodel = \"m\"").unwrap();
        assert_eq!(c.roles, vec!["worker"]);
        assert!(c.enabled);
        assert_eq!(c.effort, None);
    }

    #[test]
    fn event_round_trip_and_ids() {
        let e = Event {
            id: new_id(),
            session_id: new_id(),
            run_id: None,
            ts: Utc::now(),
            event_type: EventType::StateChanged,
            data: serde_json::json!({"from": "queued", "to": "starting"}),
        };
        let j = serde_json::to_string(&e).unwrap();
        assert!(j.contains("\"type\":\"state_changed\""));
        assert_eq!(serde_json::from_str::<Event>(&j).unwrap(), e);
        assert_ne!(new_id(), new_id());
    }
}

#[cfg(test)]
mod headroom_wire_tests {
    use super::*;

    fn headroom(observed_at: DateTime<Utc>) -> Headroom {
        Headroom {
            account: "a".into(),
            pool: "p".into(),
            five_hour_remaining_pct: None,
            five_hour_resets_at: None,
            weekly_remaining_pct: None,
            weekly_resets_at: None,
            status: HeadroomStatus::Unknown,
            source: HeadroomSource::Ledger,
            observed_at,
        }
    }

    #[test]
    fn never_observed_is_null_on_the_wire_and_round_trips() {
        let v = serde_json::to_value(headroom(DateTime::UNIX_EPOCH)).unwrap();
        assert!(v["observed_at"].is_null(), "{v}");
        let back: Headroom = serde_json::from_value(v).unwrap();
        assert_eq!(back.observed_at, DateTime::UNIX_EPOCH);
    }

    #[test]
    fn a_real_timestamp_is_kept() {
        let t = Utc::now();
        let v = serde_json::to_value(headroom(t)).unwrap();
        assert!(v["observed_at"].is_string(), "{v}");
        let back: Headroom = serde_json::from_value(v).unwrap();
        assert_eq!(back.observed_at, t);
    }
}
