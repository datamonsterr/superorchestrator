//! Deterministic, independently applicable suggestions for level configuration.
use crate::{
    catalog::{Catalog, Tier},
    insights::{BOUNDARY_BAND, HistoryRecord, LevelInsight},
    levels::{levels_by_distance, tier_of},
};
use serde::Serialize;
use so_core::config::{Level, validate_levels};
use std::collections::BTreeMap;

pub const MIN_SAMPLES: usize = 5;
pub const BAD_RATE: f64 = 0.5;
pub const SHIFT: u8 = 5;
pub const PROMOTE_MARGIN: f64 = 0.2;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Rule {
    ShiftBoundary,
    PromoteModel,
    AssignModel,
    CloseGap,
    RemoveEmpty,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LevelChange {
    pub level: String,
    pub field: String,
    pub before: String,
    pub after: String,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Suggestion {
    pub id: String,
    pub rule: Rule,
    pub level: String,
    pub rationale: String,
    pub levels: Vec<Level>,
    pub diff: Vec<LevelChange>,
}

fn change(level: &Level, field: &str, before: impl ToString, after: impl ToString) -> LevelChange {
    LevelChange {
        level: level.name.clone(),
        field: field.into(),
        before: before.to_string(),
        after: after.to_string(),
    }
}
fn models(l: &Level) -> String {
    l.models
        .iter()
        .map(|c| c.id())
        .collect::<Vec<_>>()
        .join(", ")
}
fn push(
    out: &mut Vec<Suggestion>,
    id: String,
    rule: Rule,
    level: &Level,
    rationale: String,
    levels: Vec<Level>,
    diff: Vec<LevelChange>,
) {
    if validate_levels(&levels).is_ok() {
        out.push(Suggestion {
            id,
            rule,
            level: level.name.clone(),
            rationale,
            levels,
            diff,
        });
    }
}
fn level_tier(l: &Level, catalog: &Catalog) -> Option<Tier> {
    if let Some(t) = l.tier {
        return Some(tier_of(t));
    }
    let mut counts = [0; 3];
    for c in &l.models {
        if let Some(m) = catalog.get(c.harness, &c.model) {
            counts[match m.tier {
                Tier::High => 0,
                Tier::Mid => 1,
                Tier::Fast => 2,
            }] += 1;
        }
    }
    let max = *counts.iter().max()?;
    if max == 0 {
        None
    } else {
        Some([Tier::High, Tier::Mid, Tier::Fast][counts.iter().position(|&c| c == max)?])
    }
}
fn shift_boundary(
    levels: &[Level],
    order: &[usize],
    records: &[HistoryRecord],
    out: &mut Vec<Suggestion>,
) {
    for pair in order.windows(2) {
        let (i, j) = (pair[0], pair[1]);
        let (a, b) = (&levels[i], &levels[j]);
        let Some(new) = a.max.checked_sub(SHIFT).filter(|&n| n >= a.min) else {
            continue;
        };
        let rows: Vec<_> = records
            .iter()
            .filter(|r| {
                r.complexity >= f64::from(a.max) + 1.0 - BOUNDARY_BAND
                    && levels_by_distance(levels, r.complexity).first() == Some(&i)
            })
            .collect();
        let bad = rows.iter().filter(|r| r.bad()).count();
        if rows.len() < MIN_SAMPLES || (bad as f64 / rows.len() as f64) < BAD_RATE {
            continue;
        }
        let mut next = levels.to_vec();
        next[i].max = new;
        let mut diff = vec![change(a, "max", a.max, new)];
        if u16::from(b.min) == u16::from(a.max) + 1 {
            next[j].min -= SHIFT;
            diff.push(change(b, "min", b.min, next[j].min));
        }
        push(
            out,
            format!("shift-boundary:{}:{}-{}", a.name, a.max, new),
            Rule::ShiftBoundary,
            a,
            format!(
                "{bad} of {} runs within {BOUNDARY_BAND} of `{}`'s upper bound had a bad outcome ({:.0}%); move the boundary down by {SHIFT} toward `{}`.",
                rows.len(),
                a.name,
                100.0 * bad as f64 / rows.len() as f64,
                b.name
            ),
            next,
            diff,
        );
    }
}

fn promote_model(
    levels: &[Level],
    order: &[usize],
    insights: &[LevelInsight],
    out: &mut Vec<Suggestion>,
) {
    for &i in order {
        let l = &levels[i];
        let Some(insight) = insights.iter().find(|x| x.name == l.name) else {
            continue;
        };
        let stats: Vec<_> = l
            .models
            .iter()
            .map(|c| {
                insight
                    .models
                    .iter()
                    .find(|m| m.choice_id.as_deref() == Some(c.id().as_str()))
                    .filter(|m| m.runs >= MIN_SAMPLES)
                    .and_then(|m| m.success_rate.map(|rate| (rate, m.runs)))
            })
            .collect();
        let mut best: Option<(usize, usize, f64)> = None;
        for x in 0..stats.len() {
            for y in x + 1..stats.len() {
                if let (Some((a, _)), Some((b, _))) = (stats[x], stats[y]) {
                    let gain = b - a;
                    if b >= a + PROMOTE_MARGIN && best.is_none_or(|(_, _, g)| gain > g) {
                        best = Some((x, y, gain));
                    }
                }
            }
        }
        if let Some((x, y, _)) = best {
            let (old_rate, old_runs) = stats[x].expect("chosen model has stats");
            let (new_rate, new_runs) = stats[y].expect("chosen model has stats");
            let old_id = l.models[x].id();
            let mut next = levels.to_vec();
            let c = next[i].models.remove(y);
            let id = c.id();
            next[i].models.insert(x, c);
            let diff = vec![change(l, "models", models(l), models(&next[i]))];
            push(
                out,
                format!("promote-model:{}:{id}", l.name),
                Rule::PromoteModel,
                l,
                format!(
                    "`{id}` ({:.0}% success, {new_runs} runs) vs `{old_id}` ({:.0}% success, {old_runs} runs) in `{}`; move `{id}` ahead of `{old_id}`.",
                    new_rate * 100.0,
                    old_rate * 100.0,
                    l.name
                ),
                next,
                diff,
            );
        }
    }
}

fn assign_model(levels: &[Level], order: &[usize], catalog: &Catalog, out: &mut Vec<Suggestion>) {
    let used: Vec<_> = levels
        .iter()
        .flat_map(|l| l.models.iter().cloned())
        .collect();
    for entry in crate::catalog::unassigned_models(catalog, &used) {
        let m = catalog
            .get(entry.harness, &entry.model)
            .expect("catalog entry exists");
        let matching: Vec<_> = order
            .iter()
            .copied()
            .filter(|&i| level_tier(&levels[i], catalog) == Some(m.tier))
            .collect();
        if matching.is_empty() {
            continue;
        }
        let i = match m.tier {
            Tier::High => matching[matching.len() - 1],
            Tier::Fast => matching[0],
            Tier::Mid => matching[(matching.len() - 1) / 2],
        };
        let l = &levels[i];
        if l.models.is_empty() {
            continue;
        }
        let mut c = so_core::Choice {
            harness: entry.harness,
            model: entry.model.clone(),
            effort: None,
        };
        c.effort = m.efforts.last().cloned();
        let id = c.id();
        let mut next = levels.to_vec();
        next[i].models.push(c);
        let diff = vec![change(l, "models", models(l), models(&next[i]))];
        push(
            out,
            format!("assign-model:{}:{id}", l.name),
            Rule::AssignModel,
            l,
            format!(
                "Assign unlisted {0} tier model `{id}` to `{1}`.",
                m.tier.as_str(),
                l.name
            ),
            next,
            diff,
        );
    }
}

fn close_gap(levels: &[Level], order: &[usize], out: &mut Vec<Suggestion>) {
    for (pos, &i) in order.iter().enumerate() {
        let l = &levels[i];
        let mut gaps = Vec::new();
        if pos == 0 && l.min > 0 {
            gaps.push((0, l.min - 1, true));
        }
        if let Some(&j) = order.get(pos + 1) {
            if u16::from(levels[j].min) > u16::from(l.max) + 1 {
                gaps.push((l.max + 1, levels[j].min - 1, false));
            }
        } else if l.max < 100 {
            gaps.push((l.max + 1, 100, false));
        }
        for (from, to, lower) in gaps {
            let mut next = levels.to_vec();
            let diff = if lower {
                next[i].min = 0;
                vec![change(l, "min", l.min, 0)]
            } else {
                next[i].max = to;
                vec![change(l, "max", l.max, to)]
            };
            push(
                out,
                format!("close-gap:{}:{from}-{to}", l.name),
                Rule::CloseGap,
                l,
                format!("Extend `{}` to cover the gap {from}-{to}.", l.name),
                next,
                diff,
            );
        }
    }
}

fn remove_empty(levels: &[Level], order: &[usize], catalog: &Catalog, out: &mut Vec<Suggestion>) {
    for (pos, &i) in order.iter().enumerate() {
        let l = &levels[i];
        if order.len() < 2
            || l.models.is_empty()
            || l.models
                .iter()
                .any(|c| catalog.contains(c.harness, &c.model))
        {
            continue;
        }
        let neighbor = if pos > 0 { order[pos - 1] } else { order[1] };
        let n = &levels[neighbor];
        let mut next = levels.to_vec();
        let mut diff = vec![change(l, "removed", format!("{}-{}", l.min, l.max), "")];
        if pos > 0 {
            next[neighbor].max = l.max;
            diff.push(change(n, "max", n.max, l.max));
        } else {
            next[neighbor].min = l.min;
            diff.push(change(n, "min", n.min, l.min));
        }
        next.remove(i);
        push(
            out,
            format!("remove-empty:{}", l.name),
            Rule::RemoveEmpty,
            l,
            format!(
                "Remove `{}` because none of its models are in the catalog; give its range to `{}`.",
                l.name, n.name
            ),
            next,
            diff,
        );
    }
}

/// Produce suggestions, each containing a complete independently valid configuration.
pub fn suggest(
    levels: &[Level],
    insights: &[LevelInsight],
    records: &[HistoryRecord],
    catalog: &Catalog,
) -> Vec<Suggestion> {
    let mut order: Vec<usize> = (0..levels.len()).collect();
    order.sort_by_key(|&i| levels[i].min);
    let mut out = Vec::new();
    shift_boundary(levels, &order, records, &mut out);
    promote_model(levels, &order, insights, &mut out);
    assign_model(levels, &order, catalog, &mut out);
    close_gap(levels, &order, &mut out);
    remove_empty(levels, &order, catalog, &mut out);
    let mut minima = BTreeMap::new();
    for level in levels {
        minima.entry(level.name.as_str()).or_insert(level.min);
    }
    let rank = |rule: &Rule| match rule {
        Rule::ShiftBoundary => 0,
        Rule::PromoteModel => 1,
        Rule::AssignModel => 2,
        Rule::CloseGap => 3,
        Rule::RemoveEmpty => 4,
    };
    out.sort_by(|a, b| {
        rank(&a.rule)
            .cmp(&rank(&b.rule))
            .then_with(|| {
                minima
                    .get(a.level.as_str())
                    .cmp(&minima.get(b.level.as_str()))
            })
            .then(a.id.cmp(&b.id))
    });
    out
}
