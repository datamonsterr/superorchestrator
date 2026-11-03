# E4 Level insights: plan

Goal: scan models and routing history, then suggest better levels. For each level, show example tasks the decider placed there, so the operator sees what "simple", "moderate" and the other levels mean in practice.

Every task is TDD: write the failing test first. All tasks keep to `cargo check`/`cargo clippy` while iterating, then run their crate tests. Glossary terms from CONTEXT.md are used exactly (level, choice, model ref, decider, catalog, review gate).

## Design

- **History record.** One per run that has a `route_decision` event with a complexity: `run_id`, prompt, final `state`, `error_kind`, `choice_id`, `created_at`/`started_at`/`finished_at`, complexity, recorded level, route source, plus counts of `review_accepted`, `review_rejected`, `failover` and `escalated` events. Loaded by `so-store` (read-only query), analysed in `so-router` (pure functions, no I/O).
- **Outcome of a run.** `rejected` when it has at least one `review_rejected` event; else `accepted` when it has a `review_accepted` event; else `failed` when state is `failed`; else `succeeded` when state is `succeeded`; `cancelled` and non-terminal runs are excluded from rates. `failover` is a separate flag (any `failover` event). A run is a **bad outcome** when it was rejected, failed, escalated or failed over. **Success rate** of a model = share of its counted runs that are not bad.
- **Level of a record.** Map the recorded complexity against the levels in effect now (`so_router::levels::levels_by_distance`), not the recorded level name, so suggestions match the current config.
- **Excerpt.** `so_core::redact::prompt_excerpt(text, max_chars)`: collapse whitespace, redact with `so_core::redact::redact`, truncate on a char boundary to `max_chars` with a trailing `…`. Default 160 characters.
- **Suggestions** are deterministic. Constants (in `so-router/src/suggest.rs`): `MIN_SAMPLES = 5`, `BOUNDARY_BAND = 5`, `BAD_RATE = 0.5`, `SHIFT = 5`, `PROMOTE_MARGIN = 0.2`. Each suggestion: stable readable `id` (for example `shift-boundary:simple:37-32`, `promote-model:moderate:codex/gpt-6-luna/medium`, `assign-model:complex:codex/gpt-6.2/high`, `close-gap:moderate:62-70`, `remove-empty:tiny`), `rule`, `rationale` (one sentence with the numbers), `levels` (the complete proposed `router.levels`) and `diff` (list of `{level, field, before, after}`). Every proposed `levels` passes `Config` level validation; a rule that cannot produce valid levels emits nothing.
- **Apply** always goes through the existing levels write path: the CLI through `Config::set_many_in_file(router.levels)` (as `cmd/levels.rs` `Draft::save`), the web through the existing `PUT /api/v1/levels` with the suggestion's `levels`.

## Tasks

### Task 1: Route log prompt excerpt fix and shared excerpt helper

Goal: `GET /api/v1/routing/records` returns a real `prompt_excerpt`. Production never writes `data["prompt_excerpt"]` into `route_decision` events, so excerpts are always empty.
- Add `so_core::redact::prompt_excerpt(text: &str, max_chars: usize) -> String` (see Design) with unit tests: whitespace collapse, secret redaction, char-boundary truncation of multibyte text, no ellipsis when short.
- In `crates/so-web/src/api/system.rs` `routing_records`, take the excerpt from `runs.prompt` of the event's `run_id` (one batched store lookup for the page, not one query per event), through `prompt_excerpt(.., 160)`. Keep a fallback to `data["prompt_excerpt"]` when the run row has no prompt.
- Fix the test fixture at `crates/so-web/tests/api.rs` (around line 165) so it seeds a run with a prompt instead of writing `prompt_excerpt` into the event, and add a regression test that a route_decision event without `prompt_excerpt` yields the run's redacted, truncated prompt.
Acceptance: `cargo test -p so-core redact`, `cargo test -p so-web`, `cargo clippy -p so-core -p so-web --all-targets -- -D warnings`.
Out of scope: any other endpoint, web UI changes, analyzer.

### Task 2: Extensible model scan with Claude and unassigned models

Goal: the model scan covers every installed harness that can report models, including Claude, and is easy to extend for the harnesses E2 and E3 add.
- In `crates/so-router/src/catalog.rs` `refresh`, replace the hard-coded `tokio::join!` of four gatherers with a small table of model sources (one entry per harness or service; each says how to collect: codex app-server, CLI listing, models.dev, or a static documented list). Adding a harness should mean adding one entry. Keep behaviour and reports of the existing sources unchanged.
- Claude: `claude` (Claude Code 2.1.x) has no model-listing command (`claude --help` lists only `--model`). Add a documented static source for `Harness::Claude` with ids `claude-opus-5-5`, `claude-sonnet-5-5`, `claude-haiku-5-5`, `claude-fable-5-1`, used only when an enabled claude account exists; report it as a source named `claude` with detail `documented model ids (claude has no model listing)`. New ids get tier from `infer_tier` and are merged like other sources.
- Rename `RefreshReport.unused_models` to `unassigned_models` (catalog models no level lists) and keep a serde alias only if something deserialises it. Update `crates/superorchestrator/src/cmd/models.rs` refresh output wording ("not listed by any level") and add a `LEVELS` column to `models list` (names of levels whose models list the catalog model, any effort; empty when none). Update `crates/so-supervisor/src/services/models.rs` and the web API view types if they carry the field (do not regenerate `web/src/api/generated.ts` here unless the `so-web` tsgen test requires it; if it does, regenerate with `cargo run -p so-web --bin gen-ts`).
Acceptance: `cargo test -p so-router catalog`, `cargo test -p superorchestrator models`, `cargo test -p so-web tsgen`, `cargo clippy -p so-router -p superorchestrator -p so-web --all-targets -- -D warnings`.
Out of scope: insights, suggestions, web panels, harness drivers of E2/E3.

### Task 3: Routing history loader and insights analyzer

Depends on Task 1 (uses `prompt_excerpt`).
Goal: per-level statistics and example tasks from routing history.
- `so-store`: new read-only `Store::routing_history(limit: usize) -> Result<Vec<HistoryRow>>` (new file `crates/so-store/src/history.rs`) returning the history record of Design, newest first, joining `runs` with the latest `route_decision` event per run and counting `review_accepted`, `review_rejected`, `failover`, `escalated` events. One SQL query or a small fixed number, no per-run queries. Tests against a fixture DB built with the store's own insert APIs.
- `so-router`: new `crates/so-router/src/insights.rs` with pure `analyze(levels: &[Level], records: &[HistoryRecord], eval: &[EvalExample]) -> Vec<LevelInsight>`. Per level: `runs`, complexity distribution (`min`, `median`, `max`, and counts in 10-point buckets inside the level), outcome counts and rates (accepted, rejected, failed, succeeded, failover), `median_duration_ms` (finished minus started), per-model stats (`choice_id`, runs, success rate), and `examples`: up to 3 nearest the level centre plus up to 2 borderline (within `BOUNDARY_BAND` of either bound), each with `run_id`, `excerpt` (`prompt_excerpt(.., 160)`), complexity, outcome, `source: "history"`. When a level has fewer than 3 history examples, fill up to 3 from the eval sets (`so-router/evals/*.jsonl`, items whose `expected_complexity` midpoint falls in the level) with `source: "eval"` and the eval `id`. Expose an `eval_examples()` loader for the bundled eval files (reuse `eval.rs` parsing).
- `HistoryRecord` lives in `so-router` (or `so-core` if both crates need it); `so-store` returns rows that convert to it. All types `Serialize` (and `schemars`/ts-export the way `so-web/src/types.rs` expects later).
Tests: aggregates (counts, median, rates, per-model), example selection (centre, borderline, redaction, truncation, eval fallback labelled `eval`), mapping by current levels.
Acceptance: `cargo test -p so-store history`, `cargo test -p so-router insights`, `cargo clippy -p so-store -p so-router --all-targets -- -D warnings`.
Out of scope: suggestions, CLI, web, TUI.

### Task 4: Suggestion rules

Depends on Task 3.
Goal: `crates/so-router/src/suggest.rs` with pure `suggest(levels: &[Level], insights: &[LevelInsight], records: &[HistoryRecord], catalog: &Catalog) -> Vec<Suggestion>` following Design. Rules:
1. `shift-boundary`: for adjacent levels A (lower) and B, take runs of A with complexity in `[A.max - BOUNDARY_BAND + 1, A.max]`. When there are at least `MIN_SAMPLES` and the bad-outcome rate is at least `BAD_RATE`, lower the boundary by `SHIFT` (A.max -= SHIFT, B.min -= SHIFT), keeping A at least 1 wide.
2. `promote-model`: in a level that lists models, when a later model has at least `MIN_SAMPLES` runs in the level and its success rate exceeds an earlier model's (also at least `MIN_SAMPLES`) by at least `PROMOTE_MARGIN`, move it directly ahead of that model. One suggestion per level (the largest gain).
3. `assign-model`: for each catalog model no level lists, add `harness/model` (with the catalog entry's highest effort when it has efforts) to the end of the level whose tier matches: the level's `tier`, else the majority catalog tier of its listed models. High tier goes to the highest such level, fast to the lowest, mid to the middle one. Skip when no level matches.
4. `close-gap`: for a range of complexities no level covers, extend the lower neighbour's `max` to close it. `remove-empty`: a level that lists no models and has no tier, or lists only refs the catalog does not know, is removed and its range given to the lower neighbour (or upper when it is the lowest). Overlaps are already rejected by config validation, so no overlap rule.
Each suggestion validates its proposed levels with the same validation `Config` uses (expose a public `validate_levels` from `so-core` config if it is private) and is dropped when invalid. Ids are stable for the same inputs. Sorted by rule order, then level order.
Tests: each rule fires at its threshold and not one sample or one point below it; minimum sample size; ids stable; every emitted `levels` passes validation; a round-trip test writes the suggestion's levels with `Config::set_many_in_file` into a temp config and reloads it successfully.
Acceptance: `cargo test -p so-router suggest`, `cargo test -p so-core`, `cargo clippy -p so-router -p so-core --all-targets -- -D warnings`.
Out of scope: CLI, web, TUI, LLM suggestions.

### Task 5: Insights service, CLI `levels suggest`, web API

Depends on Tasks 2-4.
Goal: one service builds the insights report; CLI and web expose it.
- `crates/so-supervisor/src/services/insights.rs`: `insights_report(paths, store, config, catalog) -> InsightsReport { generated_at, history_runs, levels: Vec<LevelInsight>, suggestions: Vec<Suggestion>, unassigned_models }` (history limit 5000).
- CLI: `superorchestrator levels suggest [--apply <id>]` in `cli.rs` / `cmd/levels.rs`. Without `--apply`: a readable report (per level: range, runs, outcome rates, median duration, top models, examples with source label; then numbered suggestions with id, rationale and diff) and `--json` support like other commands. With `--apply <id>`: recompute, find the id (error listing valid ids when unknown), write through the same path as `Draft::save` (validation, "restart required" note). It reads the DB path from the normal paths (`ctx`); it must also work against a DB copy chosen with the existing config-dir override used in tests.
- Web: `GET /api/v1/routing/insights` returning `InsightsReport` in `crates/so-web/src/api/system.rs` and the router in `api/mod.rs`; add the types to `so-web` types/tsgen and regenerate `web/src/api/generated.ts` with `cargo run -p so-web --bin gen-ts`. Apply in the web uses the existing `PUT /api/v1/levels` (no new write endpoint).
Tests: CLI test with a fixture DB (report text and `--json`, `--apply` writes valid levels, unknown id fails), `so-web/tests` API test for the endpoint, tsgen test passes.
Acceptance: `cargo test -p so-supervisor insights`, `cargo test -p superorchestrator levels`, `cargo test -p so-web`, `cargo clippy --workspace --all-targets -- -D warnings`.
Out of scope: web UI, TUI.

### Task 6: Web Insights panel

Depends on Task 5.
Goal: the Routing page (`web/src/features/routing/RoutingPage.tsx`) gets an Insights panel: per-level stats (runs, complexity range and distribution, outcome rates, median duration, per-model success), example tasks with a "history"/"eval" label, and suggestions with rationale, the diff and an Apply button that sends the suggestion's `levels` through the existing levels PUT mutation and refreshes levels and insights. `ModelsPanel` marks unassigned models. Add the endpoint to `web/src/api/endpoints.ts`/`queries.ts`, an MSW fixture in `web/src/api/msw`, vitest tests, and a Playwright case in `web/e2e/smoke.spec.ts` (mock mode) that opens Routing, sees the Insights panel with an example and applies a suggestion.
Acceptance: `pnpm -C web lint`, `pnpm -C web typecheck`, `pnpm -C web test`, `pnpm -C web build`, `pnpm -C web size`, `pnpm -C web e2e`.
Out of scope: other pages, decider settings.

### Task 7: TUI Levels tab insights

Depends on Task 5.
Goal: the TUI Levels tab (`crates/superorchestrator/src/tui/`) shows, for the selected level, its insight stats and example tasks, and a suggestions list with an apply key (with y/n confirm, like reset) that writes through the same path as `levels suggest --apply`. Insights load with the existing snapshot/data flow (`tui/data.rs`). Tests in the `tui/tests.rs` style (key handling, render buffer contains stats, examples and suggestion; apply effect).
Acceptance: `cargo test -p superorchestrator tui`, `cargo clippy -p superorchestrator --all-targets -- -D warnings`.
Out of scope: new TUI tabs (E5 adds them).

### Task 8: Docs

Depends on Tasks 1-5 (describes their behaviour).
Goal: SPEC.md (insights, suggestions, endpoint, CLI), ADR `docs/adr/0008-level-suggestion-rules.md` (rules, thresholds, outcome definition, why deterministic), `docs/guide/routing.md` section "Level insights and suggestions" (plus short notes in `web-ui.md` and `tui.md`), CONTEXT.md terms (level insight, suggestion, bad outcome) and CHANGELOG.
Acceptance: `sh scripts/check-docs.sh` if present, else `git diff --check`.
Out of scope: code changes.

## Order

Tasks 1 and 2 in parallel. Then 3, 4, 5. Then 6, 7 and 8 in parallel. Final whole-branch quality review, `cargo test --workspace`, real check against a DB copy, screenshots, PR.
