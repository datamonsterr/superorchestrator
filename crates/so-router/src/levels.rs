//! Levels: which level a complexity falls in, and which choice of that level wins.
//! Pure over the eligible candidates; the account is chosen later by `select_account`.

use so_core::config::{Level, LevelTier, TaskFamily};

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

pub fn tier_of(t: LevelTier) -> Tier {
    match t {
        LevelTier::High => Tier::High,
        LevelTier::Mid => Tier::Mid,
        LevelTier::Fast => Tier::Fast,
    }
}

/// Upper end of a level's span: exclusive `max + 1`, except that `max = 100` includes 100.0.
fn upper(l: &Level) -> f64 {
    if l.max >= 100 {
        100.0
    } else {
        f64::from(l.max) + 1.0
    }
}

fn contains(l: &Level, complexity: f64) -> bool {
    complexity >= f64::from(l.min)
        && (complexity < upper(l) || (l.max >= 100 && complexity <= 100.0))
}

/// 0 inside the span, else the distance to the nearest bound.
fn distance(l: &Level, complexity: f64) -> f64 {
    if contains(l, complexity) {
        0.0
    } else if complexity < f64::from(l.min) {
        f64::from(l.min) - complexity
    } else {
        complexity - upper(l)
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

/// Candidates a level allows: if a task family is given and the level configures preferred
/// models for it, those eligible choices come first (in preference order). Next come the
/// remaining eligible choices of the level (in `models` list order, or tier order).
fn level_candidates(
    level: &Level,
    cands: &[Candidate<'_>],
    family: Option<TaskFamily>,
) -> Vec<usize> {
    let mut out = Vec::new();
    if let Some(f) = family
        && let Some(prefs) = level.families.get(&f)
    {
        for m in prefs {
            if let Some(i) = cands.iter().position(|c| &c.choice == m)
                && !out.contains(&i)
            {
                out.push(i);
            }
        }
    }
    if level.models.is_empty() {
        let Some(tier) = level.tier else {
            return out;
        };
        for (i, cand) in cands.iter().enumerate() {
            if cand.tier == tier_of(tier) && !out.contains(&i) {
                out.push(i);
            }
        }
    } else {
        for m in &level.models {
            if let Some(i) = cands.iter().position(|c| &c.choice == m)
                && !out.contains(&i)
            {
                out.push(i);
            }
        }
    }
    out
}

/// Best account's headroom for the pool and its active runs.
fn load(c: &Candidate<'_>) -> (f64, u32) {
    select_account(&c.accounts, &c.pool).map_or((f64::MIN, u32::MAX), |a| {
        (headroom_of(a, &c.pool), a.active)
    })
}

/// Orders a level's candidates. A level that lists models keeps its list order (position is a
/// total order). A tier level has no list: most headroom first, then fewest active runs, then
/// config order (the candidates' own order). If family preferences applied on a tier level,
/// those preferred choices stay at the front.
fn rank(
    level: &Level,
    mut idx: Vec<usize>,
    cands: &[Candidate<'_>],
    family: Option<TaskFamily>,
) -> Vec<usize> {
    if !level.models.is_empty() {
        return idx;
    }
    let family_count = family
        .and_then(|f| level.families.get(&f))
        .map_or(0, |prefs| {
            idx.iter()
                .take_while(|&&i| prefs.contains(&cands[i].choice))
                .count()
        });
    if family_count > 0 && family_count < idx.len() {
        let (_preferred, rest) = idx.split_at_mut(family_count);
        rest.sort_by(|&a, &b| {
            let ((ha, aa), (hb, ab)) = (load(&cands[a]), load(&cands[b]));
            hb.total_cmp(&ha).then(aa.cmp(&ab)).then(a.cmp(&b))
        });
        return idx;
    } else if family_count == 0 {
        idx.sort_by(|&a, &b| {
            let ((ha, aa), (hb, ab)) = (load(&cands[a]), load(&cands[b]));
            hb.total_cmp(&ha).then(aa.cmp(&ab)).then(a.cmp(&b))
        });
    }
    idx
}

fn why_won(
    level: &Level,
    ranked: &[usize],
    cands: &[Candidate<'_>],
    family: Option<TaskFamily>,
) -> String {
    if let Some(f) = family
        && let Some(prefs) = level.families.get(&f)
        && prefs.contains(&cands[ranked[0]].choice)
    {
        return format!("preferred for {}", f.as_str());
    }
    if ranked.len() == 1 {
        return "the only eligible choice of the level".into();
    }
    if !level.models.is_empty() {
        return "first in the level".into();
    }
    let (headroom, active) = load(&cands[ranked[0]]);
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
    family: Option<TaskFamily>,
) -> Selection {
    let by_distance = levels_by_distance(levels, complexity);
    let mapped = by_distance.first().map(|&i| levels[i].name.clone());
    for &li in &by_distance {
        let level = &levels[li];
        let ranked = rank(level, level_candidates(level, cands, family), cands, family);
        if ranked.is_empty() {
            continue;
        }
        let why = why_won(level, &ranked, cands, family);
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
            models: Vec::new(),
            families: std::collections::BTreeMap::new(),
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
        assert_eq!(names(&d, 37.0)[0], "low");
        assert_eq!(names(&d, 38.0)[0], "mid");
        assert_eq!(names(&d, 62.0)[0], "mid");
        assert_eq!(names(&d, 75.0)[0], "high");
        assert_eq!(names(&d, 100.0)[0], "high");
    }

    #[test]
    fn gap_goes_to_the_nearest_bound_and_ties_to_the_higher_level() {
        let g = vec![lv("a", 0, 20), lv("b", 40, 60), lv("c", 90, 100)];
        assert_eq!(names(&g, 25.0), vec!["a", "b", "c"]);
        assert_eq!(names(&g, 35.0), vec!["b", "a", "c"]);
        // 30.5 is 9.5 from both a (ends at 21) and b: the higher level wins.
        assert_eq!(names(&g, 30.5)[0], "b");
        assert_eq!(names(&g, 76.0)[0], "c");
        // Declaration order does not matter.
        let r = vec![lv("c", 90, 100), lv("a", 0, 20), lv("b", 40, 60)];
        assert_eq!(names(&r, 30.5)[0], "b");
    }

    #[test]
    fn fractional_complexity_reads_integer_ranges_as_half_open() {
        let d = Config::default().router.levels;
        assert_eq!(names(&d, 37.5)[0], "low");
        assert_eq!(names(&d, 38.0)[0], "mid");
        assert_eq!(names(&d, 62.9)[0], "mid");
        assert_eq!(names(&d, 63.0)[0], "high");
        assert_eq!(names(&d, 100.0)[0], "high");
        // Gap 21..40: 21.0 is nearest to a's end.
        let g = vec![lv("a", 0, 20), lv("b", 40, 60)];
        assert_eq!(names(&g, 20.9)[0], "a");
        assert_eq!(names(&g, 21.0)[0], "a");
    }

    #[test]
    fn no_levels_means_no_level() {
        assert!(levels_by_distance(&[], 50.0).is_empty());
    }

    #[test]
    fn task_family_preferred_model_ranks_first_within_level() {
        use crate::catalog::Tier;
        use crate::eligible::AccountState;
        use so_core::{AccountFile, Choice, Harness};

        let choice_a: Choice = "codex/m-a/high".parse().unwrap();
        let choice_b: Choice = "codex/m-b/high".parse().unwrap();
        let choice_c: Choice = "codex/m-c/high".parse().unwrap();

        let acct_file = AccountFile::new("acc", Harness::Codex, "/h", "codex").unwrap();
        let acct_state = AccountState {
            file: acct_file,
            active: 0,
            last_attempt: None,
            pools: std::collections::BTreeMap::new(),
        };
        let cand = |c: Choice| Candidate {
            choice: c,
            tier: Tier::High,
            cost: crate::catalog::Cost::Included,
            pool: "default".into(),
            accounts: vec![&acct_state],
        };

        let cands = vec![
            cand(choice_a.clone()),
            cand(choice_b.clone()),
            cand(choice_c.clone()),
        ];

        let mut level = Level {
            name: "high".into(),
            min: 63,
            max: 100,
            tier: None,
            models: vec![choice_a.clone(), choice_b.clone(), choice_c.clone()],
            families: std::collections::BTreeMap::new(),
        };
        level
            .families
            .insert(TaskFamily::CodeReview, vec![choice_b.clone()]);

        let levels = vec![level];
        let fallback = vec![0, 1, 2];

        // With code_review: choice_b (idx 1) is preferred and wins
        let sel = select(
            &levels,
            &cands,
            85.0,
            &fallback,
            Some(TaskFamily::CodeReview),
        );
        assert_eq!(sel.order[0], 1); // choice_b
        assert_eq!(sel.order[1], 0); // choice_a
        assert_eq!(sel.order[2], 2); // choice_c
        assert_eq!(sel.why, "preferred for code_review");

        // Without task family (None): level order choice_a (idx 0) wins
        let sel_none = select(&levels, &cands, 85.0, &fallback, None);
        assert_eq!(sel_none.order[0], 0); // choice_a
        assert_eq!(sel_none.why, "first in the level");

        // With unconfigured task family (Debugging): falls back to level order
        let sel_debug = select(
            &levels,
            &cands,
            85.0,
            &fallback,
            Some(TaskFamily::Debugging),
        );
        assert_eq!(sel_debug.order[0], 0); // choice_a
        assert_eq!(sel_debug.why, "first in the level");

        // When preferred choice is not eligible (only cands 0 and 2 available):
        let cands_no_b = vec![cand(choice_a.clone()), cand(choice_c.clone())];
        let fallback_no_b = vec![0, 1];
        let sel_fallback = select(
            &levels,
            &cands_no_b,
            85.0,
            &fallback_no_b,
            Some(TaskFamily::CodeReview),
        );
        assert_eq!(sel_fallback.order[0], 0); // choice_a
        assert_eq!(sel_fallback.why, "first in the level");
    }
}
