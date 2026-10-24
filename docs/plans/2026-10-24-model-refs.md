# Plan: levels list model refs, agent profiles replace roles

Date: 2026-10-24. Branch: `claude/simplify-choices-config-102252`.

## Goal

Replace the `[[choices]]` table (id, harness, model, effort, roles, description, enabled) with
one notation, the **model ref** `harness/model[/effort]`, listed directly on each
`[[router.levels]]` entry in preference order. Roles disappear: the agent profile
(`spawn_agent(agent: "worker")`, a section 10 agent file) is orthogonal to the model and any
profile may run on any model. Account selection stays a documented heuristic and is verified
for every harness. Web UI, TUI, CLI, MCP tools, skill text, docs and tests follow.

## Decisions (settled, do not reopen)

1. **Model ref** `harness/model[/effort]`. `harness` is one of `codex`, `agy`, `opencode`,
   `claude`. For `codex`, `agy` and `claude` the last `/` segment is the effort when it is one
   of `Harness::accepted_efforts()`; otherwise the whole rest is the model and effort is
   `None`. For `opencode` the rest is `provider/model[/variant]`: two segments mean no effort,
   three or more mean the last is the effort; one segment is an error ("opencode models are
   provider/model"). `agy/claude-sonnet-4-6/thinking` and `agy/claude-sonnet-4-6` are valid;
   `agy/claude-sonnet-4-6/high` is a load error (`Harness::model_effort_problem`). Model and
   effort text pass the existing GHSA-fh9p character check. Display is the canonical string;
   it is the choice id everywhere (`choice_id` in runs, events, results).
2. **`Choice`** keeps its name (glossary) and becomes `{harness, model, effort}` with
   `Choice::id()`, `FromStr`, `Display`, and string (de)serialisation. `roles`, `description`
   and `enabled` are gone. A model is enabled by being listed on a level.
3. **`Level`**: `{name, min, max, tier?, models: Vec<Choice>}`. `models` order is the
   preference order (replaces `priority`). `LevelChoice` is deleted. `tier` still applies only
   when `models` is empty. `Config.choices` field is deleted; `Config::choices()` returns the
   distinct refs of every level in level order, and `Config::lists_choice(id)` says whether a
   ref is configured. `[[choices]]` at the top level or `choices = [...]` on a level is a load
   error whose message shows the new form:
   `[[choices]] is no longer read: list models on each [[router.levels]] entry as harness/model/effort, e.g. models = ["codex/gpt-6.1-sol/high"] (docs/guide/routing.md)`.
4. **Roles are removed** from config, `RouteParams`, `SpawnParams`, the CLI (`route --role`),
   the web preview, eval cases (the key is ignored) and the skill. `RouteParams.agent:
   Option<String>` is new: the profile name, sent to the decider as `agent` (default
   `"worker"`) in place of `role`. The decider request's privacy note changes accordingly.
5. **`model` parameters accept a ref.** In `spawn_agent`, `route`, `agent()` in workflows,
   the agent file's `model:` field, `defaults.model` and `route --model`: when the value parses
   as a model ref, it pins harness, model and effort; an explicit `harness` or `effort` that
   disagrees is `INVALID_PARAMS` ("model ref names codex but harness is agy"). When it does
   not parse as a ref (for example `opencode/deepseek-v4.1-flash-free`, whose rest has one
   segment), it is a plain model name as before.
6. **Pins and claude rank unchanged in meaning.** A pinned call keeps its pick; a pinned ref
   that no level lists still runs (synthetic candidate, id = the ref). `so_core::pinned_choice_id`
   becomes `Choice::id()`; `is_pinned_choice` becomes `!config.lists_choice(id)`. Claude refs
   rank last unless named (SPEC 8.4).
7. **Account heuristic** (SPEC 8.3 step 6) stays: among the harness's enabled accounts below
   cap with the model's pool available: most headroom for the pool, then fewest active runs,
   then higher account `priority`, then least recently used, then name. The rationale now ends
   with `account <name>: <why>` (`most headroom (80%)`, `fewest active runs (0)`, `priority`,
   `least recently used`, or `the only usable account`). Tests cover all four harnesses and the
   agy pools.
8. **Starter agents** gain `worker` and `conductor` (`harness: auto`, no model pin), so
   `spawn_agent(agent: "worker", model: "codex/gpt-6.1-sol/high")` reads as the user asked.
   `ui-ux-designer` is the former designer role.
9. **Seeds**: `seed/choices.toml` is deleted; `seed/levels.toml` lists refs (mapping below);
   `config_header.toml` comments show the new form and a commented Claude line. `migrate`
   drops its `choices` step (it reports `choices: skipped, list models per level`).
10. **Web**: `GET/PUT /api/v1/choices` becomes `GET/PUT /api/v1/levels` with
    `LevelsView {levels, routing_instruction}`. The Routing page's first tab is **Levels**.
11. **Compatibility**: none for `[[choices]]`; the error message is the migration guide.

Mapping of the old seeded ids to refs (for evals, fixtures, tests, docs):

| old id | ref |
|---|---|
| agy-flash-low | agy/gemini-3.8-flash-low/low |
| agy-flash-medium, agy-designer | agy/gemini-3.8-flash-medium/medium |
| agy-flash-high | agy/gemini-3.8-flash-high/high |
| agy-sonnet-thinking | agy/claude-sonnet-4-6/thinking |
| opencode-muse-long | opencode/opencode/muse-spark-1.3 |
| opencode-deepseek-small | opencode/opencode/deepseek-v4.1-flash-free |
| codex-high | codex/gpt-6.1-sol/high |
| codex-medium | codex/gpt-6.1-sol/medium |
| claude-sonnet | claude/claude-sonnet-5-5/medium |
| claude-haiku | claude/claude-haiku-5-5/medium |

New seeded levels (`seed/levels.toml`):

```toml
[[router.levels]]
name = "trivial"
min = 0
max = 12
models = ["agy/gemini-3.8-flash-low/low", "agy/gemini-3.8-flash-medium/medium"]

[[router.levels]]
name = "simple"
min = 13
max = 37
models = ["agy/gemini-3.8-flash-medium/medium", "opencode/opencode/deepseek-v4.1-flash-free"]

[[router.levels]]
name = "moderate"
min = 38
max = 62
models = ["agy/gemini-3.8-flash-high/high", "codex/gpt-6.1-sol/medium"]

[[router.levels]]
name = "complex"
min = 63
max = 87
models = ["agy/claude-sonnet-4-6/thinking", "opencode/opencode/muse-spark-1.3", "codex/gpt-6.1-sol/high"]

[[router.levels]]
name = "expert"
min = 88
max = 100
models = ["opencode/opencode/muse-spark-1.3", "codex/gpt-6.1-sol/high", "agy/claude-sonnet-4-6/thinking"]
```

## Out of scope

- Changing the decider, anchors, complexity scoring or the heuristic scorer weights.
- New TUI screens beyond a levels summary on Overview and the route line in run details.
- Catalog (`models.toml`) format. `models refresh` keeps reporting unknown and unused models.
- Store migrations: `runs.choice_id` and `route_decision` events keep their shape.

## Tasks

Each task: TDD, `cargo check`/`cargo test` for the crates touched, one commit per task on this
branch, status report. Spec review then code quality review after each.

### T1. so-core and so-router (implementer-sonnet)

Files: `crates/so-core/src/{types.rs,config.rs,proto.rs}`, `crates/so-router/src/*`,
`crates/so-router/tests/*`, `crates/so-router/evals/*.jsonl`, `crates/so-router/catalog/default.toml`
(comment only).

- Decisions 1-7 in so-core: `Choice` parse/display/serde, `Level.models`, `Config::choices()`,
  `Config::lists_choice()`, load error for old keys, `RouteParams` (`role` out, `agent` in),
  `pinned_choice_id` removed in favour of `Choice::id()`, `is_pinned_choice` removed.
- so-router: `eligible()` takes no role; `effort_skips` and the "Skipped, invalid effort" note
  are deleted (invalid efforts cannot load). `levels.rs` ranks by list position instead of
  `priority` (`why_won`: `first in the level`, `tied ...` text goes away: position is total).
  `route.rs`: pins by ref (`parse_pins` reads `model` as a ref per decision 5), `agent` sent
  to the decider, rationale ends with the account reason (decision 7), `config_fingerprint`
  covers levels (which now hold the models), policy and instruction. `catalog.rs`:
  `unknown_choice_models`/`unused_models` take `&[Choice]` from `Config::choices()`.
  `eval.rs`: cases ignore `role`; `expected_choices` are refs. Update the three jsonl files
  with the mapping table.
- Tests: ref parsing for all four harnesses (incl. opencode 2 and 3 segments, agy thinking,
  invalid effort, unknown harness, empty model, bad characters); config load of the new
  levels, the old-key error, duplicate models in a level; routing picks by list order; a
  pinned ref outside the levels; claude rank; account heuristic tests per harness including
  agy `gemini` vs `claude` pools, cap, disabled, unknown headroom, priority, LRU.
- Done when `cargo test -p so-core -p so-router` passes. Other crates may not compile yet.

### T2. so-supervisor, so-mcp, so-agents, so-workflow (implementer-sonnet)

Files: `crates/so-supervisor/src/{runs/*,handlers/*,doctor.rs,services/models.rs}`,
`crates/so-supervisor/tests/*`, `crates/so-mcp/src/{tools.rs,compact.rs}`, `crates/so-mcp/tests/*`,
`crates/so-agents/src/{parse.rs,skill.rs,starter.rs}`, `crates/so-workflow` where it passes spawn options.

- `RoutePins.role` out, `agent` in (the agent name, for the decider). `route_run_with` builds
  `RouteParams` without role; `ChoiceFilter` ids are refs; `fallback_route` keeps working for
  a pinned harness no level lists.
- `spawn.rs`: `model` ref handling (decision 5) before the worktree is created, with the
  conflict error; the system prompt line "Carry out your role as the {} agent" stays.
- `failover.rs`: `is_pinned_choice` → `!config.lists_choice(id)`.
- `handlers/harnesses.rs` brief view: choice rows `{id, harness, model, effort?, levels: [...],
  free?, eligible?: false, why?}`; `why` drops `disabled`. Full `HarnessesView.choices` is
  `Config::choices()`.
- `doctor.rs`: level checks become: a model whose harness has no enabled account, a model not
  in the catalog (warn), `tier` ignored when `models` is set. Delete `check_old_codex_choices`
  and the effort-skips check.
- `services/models.rs`: pass `config.choices()`.
- so-mcp `PARAM_DOCS`: `route.exclude` "model refs to skip"; `spawn_agent.model`
  "model, or harness/model/effort to pin all three"; `spawn_agent.agent` "profile: worker,
  conductor, ui-ux-designer, pr-reviewer, ... or your own"; `run_workflow` description's
  `agent()` opts unchanged in names.
- so-agents: `parse.rs` accepts a ref in `model:` (decision 5; conflict with `harness:` is a
  parse error). `skill.rs` renders `## Levels` (`- name (min-max): ref, ref`) instead of
  `## Choices`, and the CLI section drops `--role`. `starter.rs` adds `worker` and
  `conductor` AGENT.md bodies (short, generic; `harness: auto`).
- Tests: supervisor integration test that routes through the supervisor for each of the four
  harnesses with fake-harness accounts, pinning by ref, and asserts `choice_id`, `harness`,
  `model`, `effort` and the account; spawn by ref on claude and agy fake accounts asserting
  the run row (codex and opencode too if `fake-harness` or `fake-codex` can stand in; say
  which in the report). Conflict error test. `list_harnesses` brief shape test. Skill render
  test. Agent file ref test.
- Done when `cargo test -p so-supervisor -p so-mcp -p so-agents -p so-workflow -p so-store` passes.

### T3. CLI crate `superorchestrator` and TUI (implementer-sonnet)

Files: `crates/superorchestrator/src/{seed/*,cmd/{init,migrate,route,models,setup,install}.rs,cli.rs,ctx.rs,tui/*}`,
`crates/superorchestrator/tests/*`.

- Seeds per decision 9. `init` writes header + levels; the "choices" wording in `init`,
  `setup` and TUI setup text becomes "config.toml, levels and catalog".
- `route`: `--role` removed; `--model REF` and `--agent NAME` added; `--explain` prints
  `accounts:` (the ranked accounts, best first) after `alternatives`.
- `migrate`: `choices` step removed per decision 9 (and its tests).
- `doctor` text and `models` report use refs.
- TUI: Overview gets a "Levels" block (one line per level: `name min-max  ref, ref, ...`,
  truncated to width) fed from `Config`; run details route line shows `source / choice_id /
  account` (model is in the ref); snapshot tests updated (`cargo insta` is not used: update
  the `.snap` files by hand or with `INSTA_UPDATE` if present).
- Tests updated for refs (route tests, init_config, seed, migrate, exec_status_doctor, tui).
- Done when `cargo test --workspace` passes.

### T4. Web backend and frontend (implementer-sonnet)

Files: `crates/so-web/src/{types.rs,api/system.rs,api/mod.rs,tsgen.rs}`, `crates/so-web/tests/api.rs`,
`web/src/api/{generated.ts,types.ts,endpoints.ts,queries.ts,keys.ts,msw/*}`,
`web/src/features/routing/*`, `web/src/features/timeline/run-panel/RouteTab.tsx`,
`web/src/components/layout/nav.ts`, `web/src/test/a11y.test.tsx`, `web/e2e/smoke.spec.ts`,
`web/e2e/real/real.spec.ts`, `web/e2e/README.md`.

- Backend: `LevelsView {levels: Vec<Level>, routing_instruction}`; `GET /levels`,
  `PUT /levels` (body `{levels, routing_instruction}`) validates by building a `Config` with
  the new levels (`Config::validate` through `parse`), writes `router.levels` and the
  instruction with `Config::set_in_file`, 400 with field errors on invalid input. Remove
  `/choices`. `HarnessesView` unchanged in name. Regenerate `generated.ts`
  (`cargo run -p so-web --bin gen-ts`).
- Frontend: Routing page tabs `Levels | Routing instruction | Preview | Route log`. Levels tab:
  a card per level (name, range, ordered model list with move up/down and remove, "Add model"
  opens a dialog: harness select, model input with catalog datalist, effort input with the
  model's efforts, live preview of the ref) and "Add level" / "Edit level" / "Delete level"
  (name, min, max; overlap and range errors shown from the server). Preview: `role` field
  removed, `model` field hint "model, or harness/model/effort". Route log and RouteTab show
  the ref as the choice. MSW fixtures and handlers follow (`/levels`).
- Tests: vitest for the levels page (render, add model, reorder, delete level, server error
  shown), schemas tests, a11y; Playwright mock smoke: open Routing, see the seeded levels,
  add a model to a level and see it listed; real spec: Routing page renders the seeded levels.
- Done when `pnpm typecheck && pnpm lint && pnpm test && pnpm e2e` pass in `web/` and
  `cargo test -p so-web` passes (the committed-TypeScript test included).

### T5. Docs (implementer-haiku)

Files: `SPEC.md` (7.2 model param, 8.2, 8.3 steps 1, 4, 5, 6, 8.4, 8.6, 10 model field, 12
`list_harnesses` shape, 13 CLI `route` flags), `CONTEXT.md` (Choice, Level definitions; add
"Model ref"; "Agent profile" under agents), `docs/guide/{routing,config,agents,web-ui,tui,migrate}.md`,
`docs/adr/0007-levels-list-model-refs.md`, `CHANGELOG.md` (Changed (breaking), Added, Removed),
`README.md` if it shows a choices example.

- Done when every `[[choices]]`, `roles`, `priority = ` (level) and `--role` mention is gone
  from the docs except the migration note, and `rg -n "roles|\[\[choices\]\]" docs SPEC.md CONTEXT.md README.md`
  shows only the CHANGELOG and the migration error text.

## Verification after the tasks

1. `cargo test --workspace`, `cargo clippy --workspace --all-targets`, `pnpm -C web test`,
   `pnpm -C web e2e`, `pnpm -C web e2e:real`.
2. Install the binary, convert `~/.config/superorchestrator/config.toml` to levels with refs,
   restart the daemon, then `superorchestrator route "<task>" --model <ref>` for one ref per
   harness (codex, agy, opencode, claude) and `doctor`.
