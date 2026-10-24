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
