//! Levels: which level a complexity falls in, and which choice of that level wins.
//! Pure over the eligible candidates; the account is chosen later by `select_account`.

use so_core::config::{Level, LevelTier};

use crate::catalog::Tier;
use crate::eligible::Candidate;
use crate::select::{headroom_of, select_account};

/// The outcome of level resolution.
#[derive(Debug, Clone, PartialEq)]
pub struct Selection {
    /// Indices into the candidates: the pick first, then the rest of its level, then every
    /// other candidate in the order given by `fallback_order`.
    pub order: Vec<usize>,
    /// Name of the level the choice came from; `None` when no level had an eligible choice.
    pub level: Option<String>,
    /// Name of the level the complexity falls in (or is nearest to); `None` without levels.
    pub mapped: Option<String>,
    /// No level had an eligible choice: every eligible choice was ranked by the heuristic.
    pub heuristic_fallback: bool,
    /// Why this choice won, for the rationale.
    pub why: String,
}

fn tier_of(t: LevelTier) -> Tier {
    match t {
        LevelTier::High => Tier::High,
        LevelTier::Mid => Tier::Mid,
        LevelTier::Fast => Tier::Fast,
    }
}

/// 0 inside the range, else the distance to the nearest bound.
fn distance(l: &Level, complexity: f64) -> f64 {
    let (min, max) = (f64::from(l.min), f64::from(l.max));
    if complexity < min {
        min - complexity
    } else if complexity > max {
        complexity - max
    } else {
        0.0
    }
}

/// Level indices ordered by distance from `complexity`, higher level first on ties. The first
/// is the level that contains it, else the one with the nearest bound.
pub fn levels_by_distance(levels: &[Level], complexity: f64) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..levels.len()).collect();
    idx.sort_by(|&a, &b| {
        distance(&levels[a], complexity)
            .total_cmp(&distance(&levels[b], complexity))
            .then(levels[b].min.cmp(&levels[a].min))
    });
    idx
}

/// Candidates a level allows: its listed ids in order, else the eligible choices of its tier
/// in config order. Both only among the eligible.
fn level_candidates(level: &Level, cands: &[Candidate<'_>]) -> Vec<usize> {
    if level.choices.is_empty() {
        let Some(tier) = level.tier else {
            return Vec::new();
        };
        return (0..cands.len())
            .filter(|&i| cands[i].tier == tier_of(tier))
            .collect();
    }
    let mut out = Vec::new();
    for lc in &level.choices {
        if let Some(i) = cands.iter().position(|c| c.choice.id == lc.id)
            && !out.contains(&i)
        {
            out.push(i);
        }
    }
    out
}

fn priority_of(level: &Level, cand: &Candidate<'_>) -> Option<u32> {
    level
        .choices
        .iter()
        .find(|c| c.id == cand.choice.id)
        .and_then(|c| c.priority)
}

/// Best account's headroom for the pool and its active runs.
fn load(c: &Candidate<'_>) -> (f64, u32) {
    select_account(&c.accounts, &c.pool).map_or((f64::MIN, u32::MAX), |a| {
        (headroom_of(a, &c.pool), a.active)
    })
}

/// Orders a level's candidates: lowest priority number first, then most headroom, then fewest
/// active runs, then config order (the candidates' own order).
fn rank(level: &Level, mut idx: Vec<usize>, cands: &[Candidate<'_>]) -> Vec<usize> {
    idx.sort_by(|&a, &b| {
        let key = |i: usize| priority_of(level, &cands[i]).unwrap_or(u32::MAX);
        let ((ha, aa), (hb, ab)) = (load(&cands[a]), load(&cands[b]));
        key(a)
            .cmp(&key(b))
            .then(hb.total_cmp(&ha))
            .then(aa.cmp(&ab))
            .then(a.cmp(&b))
    });
    idx
}

fn why_won(level: &Level, ranked: &[usize], cands: &[Candidate<'_>]) -> String {
    let first = &cands[ranked[0]];
    if ranked.len() == 1 {
        return "the only eligible choice of the level".into();
    }
    if let Some(p) = priority_of(level, first) {
        return format!("lowest priority number ({p}) in the level");
    }
    let (headroom, active) = load(first);
    format!("most headroom ({headroom:.0}%), then fewest active runs ({active})")
}

/// Resolves the level for `complexity` and picks the choice (plan "Routing procedure" steps
/// 3-5). `fallback_order` ranks every candidate by the heuristic scorer, best first; it is
/// used when no level has an eligible choice and orders the alternatives.
pub fn select(
    levels: &[Level],
    cands: &[Candidate<'_>],
    complexity: f64,
    fallback_order: &[usize],
) -> Selection {
    let by_distance = levels_by_distance(levels, complexity);
    let mapped = by_distance.first().map(|&i| levels[i].name.clone());
    for &li in &by_distance {
        let level = &levels[li];
        let ranked = rank(level, level_candidates(level, cands), cands);
        if ranked.is_empty() {
            continue;
        }
        let why = why_won(level, &ranked, cands);
        let mut order = ranked;
        let rest: Vec<usize> = fallback_order
            .iter()
            .copied()
            .filter(|i| !order.contains(i))
            .collect();
        order.extend(rest);
        return Selection {
            order,
            level: Some(level.name.clone()),
            mapped,
            heuristic_fallback: false,
            why,
        };
    }
    Selection {
        order: fallback_order.to_vec(),
        level: None,
        mapped,
        heuristic_fallback: true,
        why: "all eligible choices ranked by the heuristic scorer".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use so_core::config::Config;

    fn lv(name: &str, min: u8, max: u8) -> Level {
        Level {
            name: name.into(),
            min,
            max,
            tier: None,
            choices: Vec::new(),
        }
    }

    fn names(levels: &[Level], c: f64) -> Vec<String> {
        levels_by_distance(levels, c)
            .into_iter()
            .map(|i| levels[i].name.clone())
            .collect()
    }

    #[test]
    fn contained_complexity_picks_its_level() {
        let d = Config::default().router.levels;
        assert_eq!(names(&d, 0.0)[0], "low");
        assert_eq!(names(&d, 39.0)[0], "low");
        assert_eq!(names(&d, 40.0)[0], "mid");
        assert_eq!(names(&d, 79.0)[0], "mid");
        assert_eq!(names(&d, 100.0)[0], "high");
    }

    #[test]
    fn gap_goes_to_the_nearest_bound_and_ties_to_the_higher_level() {
        let g = vec![lv("a", 0, 20), lv("b", 40, 60), lv("c", 90, 100)];
        assert_eq!(names(&g, 25.0), vec!["a", "b", "c"]);
        assert_eq!(names(&g, 35.0), vec!["b", "a", "c"]);
        // 30 is 10 from both a and b: the higher level wins.
        assert_eq!(names(&g, 30.0)[0], "b");
        assert_eq!(names(&g, 75.0)[0], "c");
        // Declaration order does not matter.
        let r = vec![lv("c", 90, 100), lv("a", 0, 20), lv("b", 40, 60)];
        assert_eq!(names(&r, 30.0)[0], "b");
    }

    #[test]
    fn no_levels_means_no_level() {
        assert!(levels_by_distance(&[], 50.0).is_empty());
    }
}
