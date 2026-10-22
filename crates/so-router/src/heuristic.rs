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

#[derive(Debug, Clone, Copy)]
pub struct ScoreInputs {
    pub complexity: Complexity,
    pub tier: Tier,
    /// Remaining headroom of the account the choice would run on, 0..=1.
    pub headroom: f64,
    pub active_runs: u32,
    pub cost: Cost,
    pub priority: f64,
}

pub fn score(i: &ScoreInputs, w: &Weights) -> f64 {
    fit(i.complexity, i.tier) + w.headroom * i.headroom
        - w.load * f64::from(i.active_runs)
        - w.cost * cost_value(i.cost)
        + i.priority
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
