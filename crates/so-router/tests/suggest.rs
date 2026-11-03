#![allow(clippy::unwrap_used)]
use chrono::Utc;
use so_core::{
    Harness, Paths, RunState,
    config::{Config, Level, validate_levels},
};
use so_router::{
    catalog::{Catalog, Cost, HarnessModels, ModelEntry, Tier},
    insights::{HistoryRecord, ModelInsight, analyze},
    suggest::{Rule, suggest},
};

fn level(name: &str, min: u8, max: u8, models: &[&str]) -> Level {
    Level {
        name: name.into(),
        min,
        max,
        tier: None,
        models: models.iter().map(|s| s.parse().unwrap()).collect(),
    }
}
fn catalog(entries: &[(&str, Tier)]) -> Catalog {
    let mut c = Catalog::default();
    for (name, tier) in entries {
        c.harnesses
            .entry(Harness::Codex)
            .or_insert_with(HarnessModels::default)
            .models
            .push(ModelEntry {
                id: (*name).into(),
                tier: *tier,
                efforts: vec!["low".into(), "high".into()],
                pool: None,
                cost: Cost::Included,
            });
    }
    c
}
fn record(bad: bool) -> HistoryRecord {
    HistoryRecord {
        run_id: "x".into(),
        prompt: None,
        state: if bad {
            RunState::Failed
        } else {
            RunState::Succeeded
        },
        error_kind: None,
        choice_id: None,
        created_at: Utc::now(),
        started_at: None,
        finished_at: None,
        complexity: 36.0,
        level: None,
        source: None,
        reviews_accepted: 0,
        reviews_rejected: 0,
        failovers: 0,
        escalations: 0,
    }
}
fn only(
    levels: &[Level],
    records: &[HistoryRecord],
    c: &Catalog,
    rule: Rule,
) -> Vec<so_router::suggest::Suggestion> {
    let result = suggest(levels, &analyze(levels, records, &[]), records, c);
    assert_eq!(
        result,
        suggest(levels, &analyze(levels, records, &[]), records, c)
    );
    assert!(result.iter().all(|s| validate_levels(&s.levels).is_ok()));
    result.into_iter().filter(|s| s.rule == rule).collect()
}
#[test]
fn boundary_threshold_and_width() {
    let levels = vec![
        level("a", 0, 37, &["codex/a"]),
        level("b", 38, 100, &["codex/b"]),
    ];
    let c = catalog(&[("a", Tier::Fast), ("b", Tier::High)]);
    assert!(only(&levels, &vec![record(true); 4], &c, Rule::ShiftBoundary).is_empty());
    let mut rows = vec![record(false); 6];
    rows[0].state = RunState::Failed;
    rows[1].state = RunState::Failed;
    assert!(only(&levels, &rows, &c, Rule::ShiftBoundary).is_empty());
    rows[2].state = RunState::Failed;
    let s = only(&levels, &rows, &c, Rule::ShiftBoundary);
    assert_eq!(s[0].id, "shift-boundary:a:37-32");
    assert_eq!((s[0].levels[0].max, s[0].levels[1].min), (32, 33));
    let five = vec![record(true); 5];
    assert_eq!(only(&levels, &five, &c, Rule::ShiftBoundary).len(), 1);
    let mut gapped = levels.clone();
    gapped[1].min = 40;
    assert_eq!(
        only(&gapped, &five, &c, Rule::ShiftBoundary)[0].levels[1].min,
        40
    );
    let one_wide = vec![level("a", 32, 37, &["codex/a"]), levels[1].clone()];
    assert_eq!(
        only(&one_wide, &five, &c, Rule::ShiftBoundary)[0].levels[0].max,
        32
    );
    let narrow = vec![level("a", 33, 37, &["codex/a"]), levels[1].clone()];
    assert!(only(&narrow, &rows, &c, Rule::ShiftBoundary).is_empty());
}
#[test]
fn promotion_threshold_largest_gain_and_sample_count() {
    let levels = vec![level("a", 0, 100, &["codex/a", "codex/b", "codex/c"])];
    let c = catalog(&[("a", Tier::Mid), ("b", Tier::Mid), ("c", Tier::Mid)]);
    let mut insights = analyze(&levels, &[], &[]);
    insights[0].models = [("a", 5, 0.5), ("b", 5, 0.69), ("c", 4, 1.0)]
        .into_iter()
        .map(|(id, r, s)| ModelInsight {
            choice_id: Some(format!("codex/{id}")),
            runs: r,
            success_rate: Some(s),
        })
        .collect();
    assert!(
        !suggest(&levels, &insights, &[], &c)
            .iter()
            .any(|s| s.rule == Rule::PromoteModel)
    );
    insights[0].models[1].success_rate = Some(0.7);
    assert_eq!(
        suggest(&levels, &insights, &[], &c)
            .iter()
            .find(|s| s.rule == Rule::PromoteModel)
            .unwrap()
            .id,
        "promote-model:a:codex/b"
    );
    insights[0].models[2].runs = 5;
    let s = suggest(&levels, &insights, &[], &c)
        .into_iter()
        .find(|s| s.rule == Rule::PromoteModel)
        .unwrap();
    assert_eq!(s.id, "promote-model:a:codex/c");
    assert_eq!(s.levels[0].models[0].id(), "codex/c");
    assert!(s.rationale.contains("`codex/c` (100% success, 5 runs)"));
    assert!(s.rationale.contains("`codex/a` (50% success, 5 runs)"));
}
#[test]
fn assign_tiers_and_efforts() {
    let levels = vec![
        level("fast", 0, 19, &["codex/f"]),
        level("mid1", 20, 39, &["codex/m"]),
        level("mid2", 40, 59, &["codex/m"]),
        level("high", 60, 100, &["codex/h"]),
    ];
    let c = catalog(&[
        ("f", Tier::Fast),
        ("m", Tier::Mid),
        ("h", Tier::High),
        ("newf", Tier::Fast),
        ("newm", Tier::Mid),
        ("newh", Tier::High),
    ]);
    let s = only(&levels, &[], &c, Rule::AssignModel);
    for id in [
        "assign-model:fast:codex/newf/high",
        "assign-model:mid1:codex/newm/high",
        "assign-model:high:codex/newh/high",
    ] {
        assert!(s.iter().any(|x| x.id == id), "{id}");
    }
}
fn catalog_with(entries: &[(Harness, &str, Cost)]) -> Catalog {
    let mut c = Catalog::default();
    for (h, name, cost) in entries {
        c.harnesses
            .entry(*h)
            .or_insert_with(HarnessModels::default)
            .models
            .push(ModelEntry {
                id: (*name).into(),
                tier: Tier::Fast,
                efforts: vec!["low".into()],
                pool: None,
                cost: *cost,
            });
    }
    c
}
#[test]
fn assign_caps_and_orders_per_level() {
    let levels = vec![level("fast", 0, 100, &["codex/listed"])];
    let c = catalog_with(&[
        (Harness::Codex, "listed", Cost::Included),
        (Harness::Codex, "z-paid", Cost::Included),
        (Harness::Codex, "b-paid", Cost::Included),
        (Harness::Codex, "y-free", Cost::Free),
        (Harness::Opencode, "a-free", Cost::Free),
        (Harness::Opencode, "a-paid", Cost::Included),
    ]);
    let s = only(&levels, &[], &c, Rule::AssignModel);
    // Listed harness first, free before paid; the other harness is cut.
    let ids: Vec<_> = s.iter().map(|x| x.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "assign-model:fast:codex/b-paid/low",
            "assign-model:fast:codex/y-free/low",
            "assign-model:fast:codex/z-paid/low"
        ]
    );
    let more: Vec<_> = s.iter().filter(|x| x.rationale.contains("more")).collect();
    assert_eq!(more.len(), 1);
    assert!(
        more[0]
            .rationale
            .contains("2 more unlisted fast tier models fit `fast`; see unassigned_models.")
    );
    // The cut note rides on the last emitted suggestion in ranking order (z-paid).
    assert_eq!(more[0].id, "assign-model:fast:codex/z-paid/low");
}
#[test]
fn assign_free_beats_ref_order_and_small_sets_are_unchanged() {
    let levels = vec![level("fast", 0, 100, &["codex/listed"])];
    let c = catalog_with(&[
        (Harness::Codex, "listed", Cost::Included),
        (Harness::Codex, "a-paid", Cost::Included),
        (Harness::Codex, "b-free", Cost::Free),
    ]);
    let s = only(&levels, &[], &c, Rule::AssignModel);
    assert_eq!(s.len(), 2);
    assert!(s.iter().all(|x| !x.rationale.contains("more")));
}
#[test]
fn gaps_removal_and_round_trip() {
    let levels = vec![
        level("a", 10, 19, &["codex/a"]),
        level("b", 30, 49, &["codex/missing"]),
        level("c", 50, 89, &["codex/c"]),
    ];
    let c = catalog(&[("a", Tier::Fast), ("c", Tier::High)]);
    let gaps = only(&levels, &[], &c, Rule::CloseGap);
    for id in ["close-gap:a:0-9", "close-gap:a:20-29", "close-gap:c:90-100"] {
        assert!(gaps.iter().any(|x| x.id == id), "{id}");
    }
    let s = only(&levels, &[], &c, Rule::RemoveEmpty);
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].levels[0].max, 49);
    let reversed = vec![
        level("bad", 0, 19, &["codex/missing"]),
        level("good", 20, 100, &["codex/a"]),
    ];
    assert_eq!(
        only(&reversed, &[], &c, Rule::RemoveEmpty)[0].levels[0].min,
        0
    );
    assert!(only(&reversed[..1], &[], &c, Rule::RemoveEmpty).is_empty());
    let d = tempfile::tempdir().unwrap();
    let paths = Paths::from_dir(d.path().to_path_buf());
    let value = toml::Value::try_from(&s[0].levels).unwrap().to_string();
    Config::set_many_in_file(&paths, &[("router.levels", &value)]).unwrap();
    assert_eq!(Config::load(&paths).unwrap().router.levels, s[0].levels);
}
