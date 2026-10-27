#![allow(clippy::unwrap_used)]
//! Seeded level models must name models their harness lists (#7).

const SEED_LEVELS: &str = include_str!("../src/seed/levels.toml");
/// `opencode models` output, recorded in the harness crate's fixtures.
const OPENCODE_MODELS: &str = include_str!("../../so-harness/tests/fixtures/opencode/models.txt");

// Checks against the recorded `opencode models` snapshot, not the live list.
#[test]
fn seeded_opencode_models_are_listed() {
    let listed: Vec<&str> = OPENCODE_MODELS.lines().map(str::trim).collect();
    let cfg = so_core::Config::parse(SEED_LEVELS, std::path::Path::new("levels.toml")).unwrap();
    let mut checked = 0;
    for c in cfg.choices() {
        if c.harness == so_core::Harness::Opencode {
            assert!(
                listed.contains(&c.model.as_str()),
                "{} uses {}, which opencode does not list",
                c.id(),
                c.model
            );
            checked += 1;
        }
    }
    assert!(checked > 0);
}

#[test]
fn seeded_models_are_in_the_default_catalog() {
    let cfg = so_core::Config::parse(SEED_LEVELS, std::path::Path::new("levels.toml")).unwrap();
    let catalog = so_router::Catalog::default_catalog();
    for c in cfg.choices() {
        assert!(
            catalog.contains(c.harness, &c.model),
            "{} is not in the default catalog",
            c.id()
        );
    }
}

#[test]
fn gpt_6_luna_is_seeded_only_for_moderate_and_below() {
    let cfg = so_core::Config::parse(SEED_LEVELS, std::path::Path::new("levels.toml")).unwrap();
    for lv in &cfg.router.levels {
        let luna = lv.models.iter().any(|m| m.model == "gpt-6-luna");
        assert!(!luna || lv.max <= 62, "{} seeds gpt-6-luna", lv.name);
    }
}
