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

/// agy models that are the thinking variant themselves: `agy --effort` is refused for them
/// ("--effort is not supported for model"). Only the `thinking` hint (dropped at launch) or
/// no effort is valid.
pub const AGY_NO_EFFORT_MODELS: &[&str] = &["claude-sonnet-4-6", "claude-opus-4-6-thinking"];

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

    /// Like [`Harness::effort_problem`], and also refuses a real effort on an agy model that
    /// takes none (see [`AGY_NO_EFFORT_MODELS`]). The `thinking` hint stays valid there.
    pub fn model_effort_problem(self, model: &str, effort: &str) -> Option<String> {
        if let Some(p) = self.effort_problem(effort) {
            return Some(p);
        }
        if self == Harness::Agy
            && effort != EFFORT_THINKING
            && AGY_NO_EFFORT_MODELS.contains(&model)
        {
            return Some(format!(
                "effort `{effort}` is not supported for agy model `{model}` (it takes no effort; omit effort or use `{EFFORT_THINKING}`)"
            ));
        }
        None
    }

    /// The effort to pass to the harness CLI: the router hint is dropped.
    pub fn launch_effort(self, effort: Option<&str>) -> Option<&str> {
        effort.filter(|e| *e != EFFORT_THINKING)
    }
}

impl Choice {
    /// Why this choice cannot launch because of its effort, or `None`.
    pub fn effort_problem(&self) -> Option<String> {
        self.harness
            .model_effort_problem(&self.model, self.effort.as_deref()?)
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

/// One routable combination of harness, model and effort (SPEC 8.2). Its text form is the
/// model ref `harness/model[/effort]`, which is also the choice id in runs, events and results.
///
/// For `codex`, `agy` and `claude` the last `/` segment is the effort when the harness accepts
/// it as one; otherwise the whole rest is the model. For `opencode` the rest is
/// `provider/model[/variant]`: two segments are a model, a third is the effort (variant).
///
/// `Display` round-trips only for ids minted by `from_str`: a model containing `/` that is
/// pinned on codex, agy or claude may not parse back to the same choice.
#[derive(Debug, Clone, PartialEq, Eq, Hash, JsonSchema)]
#[schemars(
    with = "String",
    description = "Model ref `harness/model[/effort]`, for example `codex/gpt-6.1-sol/high` or `opencode/opencode/deepseek-v4.1-flash-free`"
)]
pub struct Choice {
    pub harness: Harness,
    pub model: String,
    pub effort: Option<String>,
}

impl Choice {
    /// The canonical ref string, used as the choice id everywhere.
    pub fn id(&self) -> String {
        self.to_string()
    }
}

impl fmt::Display for Choice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.harness, self.model)?;
        if let Some(e) = &self.effort {
            write!(f, "/{e}")?;
        }
        Ok(())
    }
}

/// The text rule for model and effort values: nonempty, no leading `-`, ASCII alphanumerics
/// or `:/_.@-[]` (GHSA-fh9p). `so-harness` applies it at launch, and refs and route pins
/// apply it earlier. `name` is `model` or `effort`, for the message.
pub fn selector_problem(name: &str, value: &str) -> Option<String> {
    let ok = !value.is_empty()
        && !value.starts_with('-')
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b":/_.@-[]".contains(&b));
    (!ok).then(|| {
        format!("invalid {name} `{value}`: expected a nonempty value without a leading '-' using only A-Za-z0-9:/_.@-[]")
    })
}

impl FromStr for Choice {
    type Err = CoreError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bad = |reason: String| CoreError::InvalidModelRef {
            value: s.to_string(),
            reason,
        };
        let (h, rest) = s
            .split_once('/')
            .ok_or_else(|| bad("expected harness/model[/effort]".into()))?;
        let harness = h
            .parse::<Harness>()
            .map_err(|_| bad(format!("unknown harness `{h}` (codex|agy|opencode|claude)")))?;
        if rest.is_empty() {
            return Err(bad("the model is empty".into()));
        }
        let segments: Vec<&str> = rest.split('/').collect();
        if segments.iter().any(|seg| seg.is_empty()) {
            return Err(bad("empty segment".into()));
        }
        let split_last = || match rest.rsplit_once('/') {
            Some((model, effort)) => (model.to_string(), Some(effort.to_string())),
            None => (rest.to_string(), None),
        };
        let (model, effort) = match harness {
            Harness::Opencode => match segments.len() {
                1 => return Err(bad("opencode models are provider/model".into())),
                2 => (rest.to_string(), None),
                _ => split_last(),
            },
            _ => {
                let last_is_effort = segments.len() > 1
                    && segments.last().is_some_and(|last| {
                        harness
                            .accepted_efforts()
                            .is_some_and(|ok| ok.contains(last))
                    });
                if last_is_effort {
                    split_last()
                } else {
                    (rest.to_string(), None)
                }
            }
        };
        if let Some(p) = selector_problem("model", &model) {
            return Err(bad(p));
        }
        if let Some(e) = &effort {
            if let Some(p) = selector_problem("effort", e) {
                return Err(bad(p));
            }
            if let Some(p) = harness.model_effort_problem(&model, e) {
                return Err(bad(p));
            }
        }
        Ok(Choice {
            harness,
            model,
            effort,
        })
    }
}

impl Serialize for Choice {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Choice {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// Whether a model id names itself free: a `-free` or `:free` suffix. The one name rule behind
/// the catalog's cost inference and the generated skill; a catalog entry with an explicit cost
/// overrides it (see `Catalog::is_free`).
pub fn model_name_is_free(model: &str) -> bool {
    model.ends_with("-free") || model.ends_with(":free")
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
    /// The run's own configuration is wrong (an unknown model): no retry on it can succeed.
    Config,
    /// The model refused the request (a provider safety filter): the account is fine and
    /// the same prompt would be refused again.
    Refused,
    Other,
}

impl ErrorKind {
    /// The failure is the account's (a limit or a login), so the account is flagged.
    /// `Config` and `Refused` failures are not the account's, and the account is fine.
    pub fn flags_account(self) -> bool {
        matches!(self, Self::Quota | Self::RateLimit | Self::Auth)
    }
}

str_enum!(ErrorKind, |s| CoreError::UnknownValue { kind: "error kind", value: s }, {
    Quota => "quota",
    RateLimit => "rate_limit",
    Auth => "auth",
    Config => "config",
    Refused => "refused",
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
    fn agy_thinking_variant_models_take_no_effort() {
        for m in AGY_NO_EFFORT_MODELS {
            let p = Harness::Agy.model_effort_problem(m, "high").unwrap();
            assert!(p.contains(m) && p.contains("high"), "{p}");
            assert!(Harness::Agy.model_effort_problem(m, "thinking").is_none());
        }
        assert!(
            Harness::Agy
                .model_effort_problem("gemini-3.8-flash-low", "high")
                .is_none()
        );
        // An invalid effort still reports the harness problem.
        assert!(
            Harness::Agy
                .model_effort_problem("claude-sonnet-4-6", "bogus")
                .unwrap()
                .contains("not valid for agy")
        );
        // Another harness with the same model name is unaffected.
        assert!(
            Harness::Claude
                .model_effort_problem("claude-sonnet-4-6", "high")
                .is_none()
        );
    }

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

    fn choice(s: &str) -> Choice {
        s.parse().unwrap_or_else(|e| panic!("{s}: {e}"))
    }

    fn ref_error(s: &str) -> String {
        s.parse::<Choice>().unwrap_err().to_string()
    }

    #[test]
    fn refs_parse_for_every_harness() {
        let cases = [
            (
                "codex/gpt-6.1-sol/high",
                Harness::Codex,
                "gpt-6.1-sol",
                Some("high"),
            ),
            ("codex/gpt-6.1-sol", Harness::Codex, "gpt-6.1-sol", None),
            (
                "claude/claude-sonnet-5-5/medium",
                Harness::Claude,
                "claude-sonnet-5-5",
                Some("medium"),
            ),
            ("claude/sonnet[1m]", Harness::Claude, "sonnet[1m]", None),
            (
                "agy/gemini-3.8-flash-low/low",
                Harness::Agy,
                "gemini-3.8-flash-low",
                Some("low"),
            ),
            (
                "agy/claude-sonnet-4-6/thinking",
                Harness::Agy,
                "claude-sonnet-4-6",
                Some("thinking"),
            ),
            (
                "agy/claude-sonnet-4-6",
                Harness::Agy,
                "claude-sonnet-4-6",
                None,
            ),
            (
                "opencode/opencode/muse-spark-1.3",
                Harness::Opencode,
                "opencode/muse-spark-1.3",
                None,
            ),
            (
                "opencode/openrouter/some-model/max",
                Harness::Opencode,
                "openrouter/some-model",
                Some("max"),
            ),
            ("opencode/a/b/c/d", Harness::Opencode, "a/b/c", Some("d")),
            ("codex/high", Harness::Codex, "high", None),
            (
                "claude/sonnet/thinking",
                Harness::Claude,
                "sonnet/thinking",
                None,
            ),
        ];
        for (text, harness, model, effort) in cases {
            let c = choice(text);
            assert_eq!(
                (c.harness, c.model.as_str(), c.effort.as_deref()),
                (harness, model, effort),
                "{text}"
            );
            assert_eq!(c.id(), text);
            assert_eq!(c.to_string(), text);
        }
    }

    #[test]
    fn invalid_refs_are_rejected() {
        let cases = [
            ("codex", "harness/model"),
            ("nope/model", "unknown harness `nope`"),
            ("codex/", "model is empty"),
            ("codex//high", "empty segment"),
            ("codex/gpt/", "empty segment"),
            ("opencode/deepseek-v4.1-flash-free", "provider/model"),
            ("agy/claude-sonnet-4-6/high", "takes no effort"),
            ("codex/gpt;rm/high", "invalid model"),
            ("codex/-gpt", "invalid model"),
            ("opencode/p/m/bad effort", "invalid effort"),
            ("codex/gpt sol", "invalid model"),
        ];
        for (text, needle) in cases {
            let e = ref_error(text);
            assert!(e.contains(needle), "{text}: {e}");
        }
    }

    #[test]
    fn choice_serde_is_the_ref_string() {
        let c = choice("codex/gpt-6.1-sol/high");
        assert_eq!(serde_json::to_value(&c).unwrap(), "codex/gpt-6.1-sol/high");
        let back: Choice = serde_json::from_str("\"agy/claude-sonnet-4-6/thinking\"").unwrap();
        assert_eq!(back.effort.as_deref(), Some("thinking"));
        assert!(serde_json::from_str::<Choice>("\"nope/x\"").is_err());
        assert!(serde_json::from_str::<Choice>("{\"harness\":\"codex\"}").is_err());
    }

    #[test]
    fn effort_problem_is_none_for_a_parsed_ref() {
        assert_eq!(
            choice("agy/claude-sonnet-4-6/thinking").effort_problem(),
            None
        );
        assert_eq!(choice("opencode/p/m/anything").effort_problem(), None);
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
