#![allow(clippy::unwrap_used)]
//! Seeded choices must name models their harness lists (#7).

const SEED_CHOICES: &str = include_str!("../src/seed/choices.toml");
/// `opencode models` output, recorded in the harness crate's fixtures.
const OPENCODE_MODELS: &str = include_str!("../../so-harness/tests/fixtures/opencode/models.txt");

// Checks against the recorded `opencode models` snapshot, not the live list.
#[test]
fn seeded_opencode_choices_use_listed_models() {
    let listed: Vec<&str> = OPENCODE_MODELS.lines().map(str::trim).collect();
    let doc: toml_edit::DocumentMut = SEED_CHOICES.parse().unwrap();
    let mut checked = 0;
    for c in doc["choices"].as_array_of_tables().unwrap() {
        if c["harness"].as_str() == Some("opencode") {
            let model = c["model"].as_str().unwrap();
            assert!(
                listed.contains(&model),
                "{} uses {model}, which opencode does not list",
                c["id"].as_str().unwrap()
            );
            checked += 1;
        }
    }
    assert!(checked > 0);
}
