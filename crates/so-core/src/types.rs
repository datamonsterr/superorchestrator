//! Glossary types shared by every crate: harnesses, runs, choices, headroom, events.

use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::CoreError;

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

impl Harness {
    pub const ALL: [Harness; 4] = [
        Harness::Codex,
        Harness::Agy,
        Harness::Opencode,
        Harness::Claude,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            Harness::Codex => "codex",
            Harness::Agy => "agy",
            Harness::Opencode => "opencode",
            Harness::Claude => "claude",
        }
    }
}

impl fmt::Display for Harness {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Harness {
    type Err = CoreError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Harness::ALL
            .into_iter()
            .find(|h| h.as_str() == s)
            .ok_or_else(|| CoreError::UnknownHarness(s.to_string()))
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
    pub fn as_str(&self) -> &'static str {
        match self {
            RunState::Queued => "queued",
            RunState::Starting => "starting",
            RunState::Running => "running",
            RunState::Waiting => "waiting",
            RunState::Stalled => "stalled",
            RunState::Escalated => "escalated",
            RunState::PendingReview => "pending_review",
            RunState::Succeeded => "succeeded",
            RunState::Failed => "failed",
            RunState::Cancelled => "cancelled",
        }
    }

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

impl fmt::Display for RunState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What kind of connection a session is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionKind {
    Orchestrator,
    Interactive,
}

fn default_roles() -> Vec<String> {
    vec!["worker".to_string()]
}

fn default_true() -> bool {
    true
}

/// One routable combination of harness, model and effort (SPEC 8.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
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

/// Where a headroom record came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HeadroomSource {
    Probe,
    Event,
    Ledger,
}

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
    pub observed_at: DateTime<Utc>,
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

/// Where a run executes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Isolation {
    Worktree,
    None,
}

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
