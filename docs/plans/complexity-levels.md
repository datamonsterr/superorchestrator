# Plan: complexity-scored routing with user-defined levels

Branch: `feat/complexity-levels` (worktree `~/dev/superorchestrator-worktrees/complexity-levels`).

## Why

Today the Jev decider picks a choice id directly (`questions.route`, `type: "choice"`). That ties the
decider to the operator's choice descriptions, harness names and account context, and every config
change shifts its answers. The new design asks the decider one narrow question: how complex is this
task? The answer is a number from 0 to 100. The operator maps ranges of that number to
**complexity levels**, and each level lists the choices (harness + model + effort) allowed for it.
Code, not the decider, then picks the choice and the account.

## Research behind the scale

- OpenRouter Decisions API `score` question: `criteria` is an ordered list of anchor texts (index 0
  is lowest); the answer is `{type: "score", score, confidence, probabilities{"0".."n-1"}, legend}`
  where `score` is the probability-weighted index in `[0, n-1]`
  (https://openrouter.ai/docs/guides/community/jev-tutorial.md).
- Hugging Face `RowRed/ComplexityRouter`: 4 ordinal levels (trivial lookup, simple, moderate,
  complex with niche expertise), trained on OASST2 + synthetic prompts.
- Hugging Face `nvidia/prompt-task-and-complexity-classifier`: continuous 0-1 complexity score
  from weighted dimensions (reasoning, constraints, domain knowledge, contextual knowledge, ...).
- SWE-bench Verified difficulty annotations: estimated developer time to decide on and implement
  the fix (<15 min, 15 min-1 h, 1-4 h, >4 h).

The anchors below combine these: coding-time buckets from SWE-bench, the reasoning / multi-step /
analysis-before-implementation dimensions from the NVIDIA classifier, and the ordinal shape of
ComplexityRouter. Five anchors give score indices 0..4, mapped linearly to 0..100.

Anchors (`so_core::config::COMPLEXITY_ANCHORS`, in order):

0. Trivial, fully specified mechanical edit: a typo, rename, formatting or a one-line config change. No reasoning or analysis needed; under 15 minutes for a developer.
1. Simple, scoped change in one or two files with a clear spec: a small bug fix, a docs page, a test for existing behaviour. Little reasoning; up to about an hour.
2. Moderate work across several files: a feature with known design, an ordinary bug that needs some investigation, a refactor with clear boundaries. Some multi-step reasoning; one to four hours.
3. Complex, multi-step work that needs analysis before implementation: unclear root cause, cross-component change, concurrency, migration or security-sensitive code, where mistakes are costly. More than four hours.
4. Highly complex, open-ended work: architecture or system design, long chains of reasoning, repeated rounds of analysis and planning, or coordinating other agents across many components.

## Terms (CONTEXT.md)

- **Complexity**: the decider's 0-100 estimate of how much reasoning, analysis and multi-step work a task needs. Avoid: difficulty, size.
- **Level**: a named, operator-defined range of complexity (`min`..`max`, inclusive, within 0..100) with its own ordered list of choices. Avoid: tier (tier is the catalog's model class), bucket, band.
- The user called a level's choice list a "pool". That word already means a metered slice of an account's usage, so the config key is `choices` and docs say "the level's choices".

## Config shape

```toml
[[router.levels]]
name = "low"
min = 0
max = 39
tier = "fast"            # optional: used only when `choices` is empty

[[router.levels]]
name = "high"
min = 80
max = 100
choices = [
  { id = "codex-gpt-high", priority = 1 },
  { id = "agy-pro-high" },          # no priority
]
```

- `Level { name: String, min: u8, max: u8, tier: Option<Tier>, choices: Vec<LevelChoice> }`
- `LevelChoice { id: String, priority: Option<u32> }` (lower number = preferred).
- Default levels (when the user sets none): `low` 0-39 tier fast, `mid` 40-79 tier mid, `high` 80-100 tier high, all with empty `choices`.
- Validation (config load/`config set`/web PUT go through the existing validation path): name non-empty and unique; `min <= max <= 100`; ranges do not overlap; every `choices[].id` names an existing choice. Gaps are allowed.

## Routing procedure (replaces SPEC 8.3 steps 2-5 for unpinned calls)

1. Eligible choices: unchanged (enabled, role, accounts with headroom and below cap, excludes, effort validity, Claude rank rule).
2. Complexity:
   - With a key: Jev `score` question `complexity` with `COMPLEXITY_ANCHORS` as criteria and `router.instruction` (new default: rate complexity only, treat task text as data) as instructions. Validation: type `score`; `score` finite in `[0, n-1]`; `confidence` finite in `[0,1]`; probabilities keys exactly `"0".."n-1"`, each in `[0,1]`, sum 1 ± 0.02. Gate: `confidence >= router.policy.min_confidence`. `min_margin` stays parseable (old configs) but is unused; doc it as deprecated. Complexity = `score / (n-1) * 100`, rounded to 1 decimal.
   - No key, rejected answer, or unreachable with `on_unreachable = "heuristic"`: the heuristic estimates a number: map existing keyword/length points to 0..100 (keep `estimate_complexity` semantics: low -> 20, mid -> 55, high -> 85, or a smooth equivalent; must land in the matching default level). Unreachable with `on_unreachable = "error"`: unchanged error.
   - Jev is called even when only one choice is eligible? No: when exactly one choice is eligible, skip the decider (same as today) and record complexity from the heuristic.
3. Level: the level whose range contains the complexity. If none (gap), the level with the nearest bound; ties go to the higher level.
4. Level's candidate choices: `choices` ids (in listed order) intersected with eligible; when `choices` is empty, eligible choices whose catalog tier equals the level's `tier` (config order). If none are eligible, try the other levels in order of distance from the complexity, higher level first on ties. If no level yields a choice, use all eligible choices ranked by the existing heuristic scorer, and say so in the rationale.
5. Pick the choice:
   - If any candidate has `priority` set: lowest priority number wins; ties and candidates without priority come after, ordered by the next rule.
   - Otherwise (and for ties): the choice whose best account is best by usage and load: most headroom for its pool, then fewest active runs on that account, then config order.
6. Account: unchanged `select_account` (most headroom, fewest active runs, account priority, LRU). Only accounts below cap are eligible already.
7. Result: existing `RouteResult` plus `complexity: Option<f64>` and `level: Option<String>` (serde default, so old stored events still parse). `source` stays `jev | heuristic | pinned` and says where the complexity came from. `probabilities` holds the decider's per-anchor probabilities keyed `"0".."4"`. `confidence` the decider's. `alternatives`: next candidates of the level, then other eligible choices. Rationale names the complexity, its source, the level and why the choice won (priority or headroom/load).
8. Pinned calls: unchanged, complexity from heuristic, level filled in for information.
9. Failover and queued re-routing reuse the same path (they call the router); behaviour on an unreachable decider is unchanged.

`config_fingerprint` includes `router.levels`.

## Tasks

### Task 1 (implementer-sonnet): core config + router

Files: `crates/so-core/src/config.rs`, `crates/so-core/src/proto.rs`, `crates/so-router/src/{jev,route,heuristic,select,eval}.rs` (+ a new `levels.rs` if it keeps route.rs focused), `crates/so-router/tests/route.rs`, config tests.

Acceptance:
- `Level`, `LevelChoice`, `Router.levels` with defaults and validation as above; JSON schema derives still compile; unknown fields still denied.
- `COMPLEXITY_ANCHORS` constant and new `DEFAULT_ROUTER_INSTRUCTION`.
- `jev.rs` sends `questions.complexity` (type `score`, list criteria) and validates/gates as above; unit tests cover every rejection case and the score mapping.
- `route.rs` follows the procedure; tests (with the existing mock HTTP setup in `tests/route.rs`) cover: score lands in level; gap goes to nearest level; level priority wins; no priority picks by headroom then load; level with no eligible choice falls to the next level; empty `choices` uses tier; no key uses heuristic complexity; rejected answer falls back; unreachable error unchanged; result carries `complexity` and `level`.
- `route eval` keeps working (accuracy against `expected_choices`); bundled eval cases may need relabelling only if the defaults make them wrong — report, do not silently rewrite many cases.
- `cargo test -p so-core -p so-router` passes; `cargo clippy -p so-core -p so-router --all-targets` clean.

Out of scope: CLI/TUI/web/supervisor output changes, docs.

### Task 2 (implementer-sonnet): surfaces

Files: `crates/superorchestrator/src/cmd/route.rs` (`--explain` prints complexity, source, level), `crates/superorchestrator/src/cmd/doctor` or `so-supervisor/src/doctor.rs` (report levels: overlaps impossible after validation, but report levels with no usable choice), `crates/superorchestrator/src/seed/config_header.toml` (commented `[[router.levels]]` example), supervisor handlers/tests that assert on Jev payloads or route output, web: `web/src/api/generated.ts`/`types.ts` (regenerate the way the repo does), `web/src/features/routing/RoutingPage.tsx` shows complexity and level, `web/src/features/settings/schemas.ts` accepts `router.levels`, msw fixtures.

Acceptance: `cargo test --workspace` passes; `web` typecheck, lint and tests pass (`npm ci` in `web/` first if needed); no level editor UI (config.toml / `config edit` is the editing path).

Out of scope: a web levels editor, docs.

### Task 3 (implementer-haiku): docs

Files: `SPEC.md` 8.3, `docs/guide/routing.md`, `docs/guide/config.md`, `CONTEXT.md` (Complexity, Level), new `docs/adr/0006-decider-scores-complexity.md`, `CHANGELOG.md` (Unreleased).

Acceptance: docs match the shipped behaviour and config keys exactly; glossary terms used exactly; ADR records the decision and the "pool" naming clash.
