//! Heuristic scorer (SPEC 8.3 step 4):
//! `score = fit + w_headroom·headroom − w_load·active_runs − w_cost·cost + priority`.

use so_core::Harness;
use so_core::config::Weights;

use crate::catalog::{Cost, Tier};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Complexity {
    Low,
    Mid,
    High,
}

impl Complexity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Mid => "mid",
            Self::High => "high",
        }
    }

    /// Numeric complexity (0 to 100) this class stands for; always inside the matching
    /// default level (low 0-39, mid 40-79, high 80-100).
    pub fn score(self) -> f64 {
        match self {
            Self::Low => 20.0,
            Self::Mid => 55.0,
            Self::High => 85.0,
        }
    }

    /// The class a numeric complexity belongs to under the default levels, read as half-open
    /// spans (low `[0, 40)`, mid `[40, 80)`, high `[80, 100]`). Ranks choices by
    /// the heuristic scorer when no level yields one.
    pub fn from_score(score: f64) -> Self {
        if score < 40.0 {
            Self::Low
        } else if score < 80.0 {
            Self::Mid
        } else {
            Self::High
        }
    }
}

const HIGH_KEYWORDS: &[&str] = &[
    "refactor",
    "architecture",
    "migrate",
    "migration",
    "concurrency",
    "concurrent",
    "race condition",
    "security",
    "authentication",
    "authorization",
    "privilege",
    "rollback",
    // Vietnamese
    "tái cấu trúc",
    "kiến trúc",
    "di chuyển dữ liệu",
    "đồng thời",
    "bảo mật",
    "phân quyền",
    "kien truc",
    "bao mat",
];

const LOW_KEYWORDS: &[&str] = &[
    "typo",
    "rename",
    "format",
    "docs",
    "documentation",
    "readme",
    // Vietnamese
    "lỗi chính tả",
    "đổi tên",
    "định dạng",
    "tài liệu",
    "chinh ta",
    "doi ten",
];

const HIGH_POINTS: i32 = 3;
const LOW_POINTS: i32 = 2;

fn length_points(chars: usize) -> i32 {
    match chars {
        4000.. => 2,
        1200..=3999 => 1,
        0..=150 => -1,
        _ => 0,
    }
}

/// Complexity from prompt length and keywords. Each distinct high keyword adds 3, each low
/// keyword subtracts 2, and length adds -1 (<=150 chars) to 2 (>=4000 chars). A total of 2
/// or more is high, below zero is low.
pub fn estimate_complexity(prompt: &str) -> Complexity {
    let p = prompt.to_lowercase();
    let count = |words: &[&str]| words.iter().filter(|w| p.contains(**w)).count() as i32;
    let total = HIGH_POINTS * count(HIGH_KEYWORDS) - LOW_POINTS * count(LOW_KEYWORDS)
        + length_points(p.chars().count());
    if total >= 2 {
        Complexity::High
    } else if total < 0 {
        Complexity::Low
    } else {
        Complexity::Mid
    }
}

/// Numeric complexity (0 to 100) estimated from the prompt, for when the decider gave none.
pub fn estimate_complexity_score(prompt: &str) -> f64 {
    estimate_complexity(prompt).score()
}

fn level(t: Tier) -> i32 {
    match t {
        Tier::Fast => 0,
        Tier::Mid => 1,
        Tier::High => 2,
    }
}

/// 1.0 when the tier matches the complexity, 0.5 one step off, 0.0 two steps off.
pub fn fit(c: Complexity, t: Tier) -> f64 {
    let want = match c {
        Complexity::Low => 0,
        Complexity::Mid => 1,
        Complexity::High => 2,
    };
    1.0 - f64::from((want - level(t)).abs()) / 2.0
}

/// Cost term: free models are negative (a bonus), included is neutral, paid is a penalty.
pub fn cost_value(c: Cost) -> f64 {
    match c {
        Cost::Free => -0.25,
        Cost::Included => 0.0,
        Cost::Paid => 1.0,
    }
}

/// Step per position in `router.priority`; the first harness gets the most.
pub const PRIORITY_STEP: f64 = 0.1;

pub fn harness_priority(order: &[Harness], h: Harness) -> f64 {
    order
        .iter()
        .position(|x| *x == h)
        .map_or(0.0, |i| (order.len() - 1 - i) as f64 * PRIORITY_STEP)
}

/// Penalty per step between the effort a task wants and the effort a choice runs at.
pub const EFFORT_STEP: f64 = 0.05;

fn effort_rank(effort: &str) -> Option<i32> {
    match effort {
        "low" | "minimal" => Some(0),
        "medium" => Some(1),
        "high" => Some(2),
        "thinking" | "xhigh" | "max" => Some(3),
        _ => None,
    }
}

/// Distance between the effort the complexity wants (medium for low and mid, high for high)
/// and the choice's effort, scaled by `EFFORT_STEP`. Zero when the choice has no effort. It
/// breaks ties between choices of one tier, so a bounded task prefers medium effort over
/// high. Low effort is never preferred: keywords cannot show that a task is trivially
/// mechanical, so the heuristic leaves that to the caller or the decider. (Measured on the
/// bundled and held-out cases: wanting low effort for low complexity cost accuracy, because
/// short prompts count as low.) A choice without an effort, or with one this does not know,
/// has gap 0: it is neither favoured nor penalised.
pub fn effort_gap(c: Complexity, effort: Option<&str>) -> f64 {
    let want = match c {
        Complexity::Low | Complexity::Mid => 1,
        Complexity::High => 2,
    };
    effort
        .and_then(effort_rank)
        .map_or(0.0, |r| f64::from((want - r).abs()) * EFFORT_STEP)
}

#[derive(Debug, Clone, Copy)]
pub struct ScoreInputs {
    pub complexity: Complexity,
    pub tier: Tier,
    /// Remaining headroom of the account the choice would run on, 0..=1.
    pub headroom: f64,
    pub active_runs: u32,
    pub cost: Cost,
    pub priority: f64,
    /// `effort_gap` of the choice.
    pub effort_gap: f64,
}

pub fn score(i: &ScoreInputs, w: &Weights) -> f64 {
    fit(i.complexity, i.tier) + w.headroom * i.headroom
        - w.load * f64::from(i.active_runs)
        - w.cost * cost_value(i.cost)
        + i.priority
        - i.effort_gap
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complexity_from_keywords_and_length() {
        assert_eq!(
            estimate_complexity("Refactor the auth module"),
            Complexity::High
        );
        assert_eq!(
            estimate_complexity("fix a typo in the readme"),
            Complexity::Low
        );
        let mid = "a".repeat(400);
        assert_eq!(estimate_complexity(&mid), Complexity::Mid);
        assert_eq!(estimate_complexity(&"x ".repeat(2500)), Complexity::High);
        assert_eq!(estimate_complexity("rename a variable"), Complexity::Low);
        // High and low cancel out on a mid-length prompt.
        let both = format!("refactor docs {}", "y".repeat(300));
        assert_eq!(estimate_complexity(&both), Complexity::Mid);
    }

    #[test]
    fn numeric_estimate_matches_the_class_and_the_default_levels() {
        let cfg = so_core::Config::default();
        for (prompt, class, level) in [
            ("fix a typo in the readme", Complexity::Low, "low"),
            (&"a".repeat(400), Complexity::Mid, "mid"),
            ("Refactor the auth module", Complexity::High, "high"),
        ] {
            let n = estimate_complexity_score(prompt);
            assert_eq!(estimate_complexity(prompt), class);
            assert_eq!(Complexity::from_score(n), class);
            let l = cfg
                .router
                .levels
                .iter()
                .find(|l| f64::from(l.min) <= n && n <= f64::from(l.max))
                .unwrap();
            assert_eq!(l.name, level);
        }
        assert_eq!(Complexity::from_score(39.9), Complexity::Low);
        assert_eq!(Complexity::from_score(39.5), Complexity::Low);
        assert_eq!(Complexity::from_score(79.9), Complexity::Mid);
        assert_eq!(Complexity::from_score(40.0), Complexity::Mid);
        assert_eq!(Complexity::from_score(80.0), Complexity::High);
    }

    #[test]
    fn vietnamese_keywords() {
        assert_eq!(
            estimate_complexity("Thiết kế lại kiến trúc dịch vụ thanh toán"),
            Complexity::High
        );
        assert_eq!(
            estimate_complexity("Sửa lỗi chính tả trong tài liệu"),
            Complexity::Low
        );
    }

    #[test]
    fn fit_matrix() {
        assert_eq!(fit(Complexity::High, Tier::High), 1.0);
        assert_eq!(fit(Complexity::High, Tier::Mid), 0.5);
        assert_eq!(fit(Complexity::High, Tier::Fast), 0.0);
        assert_eq!(fit(Complexity::Low, Tier::Fast), 1.0);
        assert_eq!(fit(Complexity::Mid, Tier::High), 0.5);
    }

    #[test]
    fn score_formula_with_weights() {
        let w = Weights {
            headroom: 2.0,
            load: 0.5,
            cost: 0.3,
        };
        let i = ScoreInputs {
            complexity: Complexity::High,
            tier: Tier::Mid,
            headroom: 0.5,
            active_runs: 2,
            cost: Cost::Paid,
            priority: 0.1,
            effort_gap: 0.0,
        };
        // 0.5 + 2*0.5 - 0.5*2 - 0.3*1 + 0.1
        assert!((score(&i, &w) - 0.3).abs() < 1e-9);
        let free = ScoreInputs {
            cost: Cost::Free,
            ..i
        };
        assert!((score(&free, &w) - score(&i, &w) - 0.375).abs() < 1e-9);
    }

    #[test]
    fn effort_gap_prefers_matching_effort() {
        let g = |c, e| effort_gap(c, Some(e));
        assert_eq!(g(Complexity::Mid, "medium"), 0.0);
        assert!(g(Complexity::Mid, "high") > 0.0);
        assert!(g(Complexity::Mid, "low") > 0.0);
        assert!(g(Complexity::Low, "medium") < g(Complexity::Low, "low"));
        assert!(g(Complexity::High, "high") < g(Complexity::High, "medium"));
        assert_eq!(effort_gap(Complexity::High, None), 0.0);
        assert_eq!(effort_gap(Complexity::High, Some("odd")), 0.0);
        // A tier step (0.5 of fit) always outweighs the effort tie-break.
        const { assert!(3.0 * EFFORT_STEP < 0.5) };
    }

    #[test]
    fn priority_follows_configured_order() {
        let order = [
            Harness::Agy,
            Harness::Opencode,
            Harness::Codex,
            Harness::Claude,
        ];
        let p = |h| harness_priority(&order, h);
        assert!(p(Harness::Agy) > p(Harness::Opencode));
        assert!(p(Harness::Opencode) > p(Harness::Codex));
        assert!(p(Harness::Codex) > p(Harness::Claude));
        assert_eq!(p(Harness::Claude), 0.0);
        assert_eq!(harness_priority(&[Harness::Codex], Harness::Agy), 0.0);
    }
}
