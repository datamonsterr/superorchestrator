# E5: decider config and web/TUI parity

Goal: make the decider (`[router.jev]` plus `[router.policy]`) configurable from the web UI and the TUI through one supervisor API, and bring the TUI to parity with the web UI on feature tabs and usage.

This epic merges first. E4 adds insights to the TUI Levels tab, so this epic does not move or restructure the Levels tab code. New TUI tabs live in their own modules under `crates/superorchestrator/src/tui/tabs/`.

## Decisions

- OpenRouter documents no list of models usable by `POST /api/alpha/decisions` (checked 2026-11-03: the API reference lists only `model: string`). The model fields are free text, and the UI offers the defaults (`inception/mercury-decide:free`, `~typesafe/jev-latest`) as suggestions.
- One decider API in the supervisor, `so_supervisor::services::decider`. The web API calls it in process (`/api/v1/decider`). The TUI calls it through new socket RPC methods (`decider_get`, `decider_set`, `decider_test`) on an inspect connection. Neither surface validates or writes config itself.
- A pasted key is written by the supervisor to `<config dir>/decider.env` as dotenv (`<key_env>=<key>`), mode 0600, then `router.jev.key_file` points at it. An ADR records this.
- Decider edits change `config.toml` on disk. "Test decider" uses the on-disk config, so it works before a restart. The view reports `restart_required` as the config view does.
- TUI tab order and names follow the web: Dashboard, Sessions, Runs, Accounts, Usage, Levels, Models, Settings, Setup. The TUI "Overview" tab is renamed "Dashboard". The web nav label "Routing" becomes "Levels" (URL unchanged). Route preview on the Levels tab is left out to avoid conflicts with E4; it is documented as a gap.

## Tasks

### Task 1: decider service, HTTP and RPC API (backend)

Goal: one supervisor API to read, change and test the decider settings.

- `so_supervisor::services::decider`:
  - `DeciderView`: `endpoint`, `model`, `fallback_model`, `on_unreachable`, `timeout`, `min_confidence`, `min_margin` (the on-disk values); `effective` (`JevModels` with sources); `env_overrides` (each env var that is set and shadows a field: var name, field, value; the key env var is listed by name only, never its value); `key` (`source` env|file|unset, `name` of the env var or file, `masked` such as `sk-or-…a1b2` showing at most the last 4 characters, or null when no key resolves); `suggestions` (the default model and fallback model); `restart_required`.
  - `DeciderPatch`: every field optional, plus `key`: one of `{ "env": "<VAR>" }`, `{ "file": "<path>" }`, `{ "paste": "<secret>" }`. Validation, each with a clear error naming the field: `endpoint` must pass `endpoint_allowed` (make it `pub` in `so-router/src/jev.rs`); `model` non-empty, no whitespace; `fallback_model` empty, `none` or a model id; `on_unreachable` `error`|`heuristic`; `timeout` a duration from 1s to 120s; `min_confidence` and `min_margin` in 0.0..=1.0; env var names `[A-Z_][A-Z0-9_]*`. Nothing is written when any field fails. Writes go through `Config::set_many_in_file`.
  - Paste: write `<config dir>/decider.env` atomically with mode 0600 (create with 0600, never chmod after writing), content `<key_env>=<secret>`, and set `router.jev.key_file` to it. The secret never appears in the view, in `GET /api/v1/config`, in logs (tracing) or in events. Any `Debug` of the patch redacts the paste.
  - `test_decider(sample prompt or a built-in sample)`: runs the decider with the on-disk config and resolved key; returns `complexity`, `level`, `confidence`, `model` used, `latency_ms`, or `error` (no key, endpoint refused, unreachable, rejected by the gate).
- HTTP in `so-web`: `GET /api/v1/decider`, `PATCH /api/v1/decider`, `POST /api/v1/decider/test`. Add the types to `gen-ts` and regenerate `web/src/api/generated.ts`.
- RPC: `METHOD_DECIDER_GET`, `METHOD_DECIDER_SET`, `METHOD_DECIDER_TEST` in `so-core` proto, handled by the supervisor for inspect connections only (run connections get an error).
- Acceptance: `cargo test -p so-supervisor -p so-web -p so-router -p so-core`; `cargo clippy -p so-supervisor -p so-web --all-targets -- -D warnings`; `cargo run -p so-web --bin gen-ts` leaves `web/src/api/generated.ts` unchanged after commit.
- Tests: PATCH validation for every field (good and bad value each); paste writes a 0600 file, the secret is absent from GET decider, GET config, captured tracing logs and stored events; masked display; env override reported; test decider against a mock decisions server (success, gate rejection, unreachable).
- Out of scope: web UI, TUI, docs, changing routing behaviour.

### Task 2: TUI tab framework and Usage tab

Goal: room for the new tabs without touching the Levels tab code, plus the Usage tab.

- `Tab` gains `Sessions`, `Usage`, `Models`, `Settings`; order Dashboard, Sessions, Runs, Accounts, Usage, Levels, Models, Settings, Setup; number keys 1-9 select them; "Overview" is renamed "Dashboard". Update the "Setup tab (N)" texts.
- The tab bar fits at 80 columns (short titles or numbered compact titles when narrow).
- New module `tui/tabs/` with `mod.rs` and one file per new tab. `ui.rs` and `app.rs` only dispatch to it. Tabs not built yet in this task (Sessions, Models, Settings) show a placeholder.
- Usage tab: per account and pool, 5-hour and weekly headroom bars with reset times ("resets in 2h 10m"), cooldowns with their end time, and a daily usage sparkline for the last 14 days from the same store data that `GET /api/v1/usage/daily` uses (call the shared function; move it into `so_supervisor::services::usage` as `pub` if needed rather than copying).
- Tests in the `tui/tests.rs` style: key handling for the new tab keys, insta snapshots of the Usage tab at 80x24 and 120x40, empty Usage state. Update existing snapshots for the tab bar change.
- Acceptance: `cargo test -p superorchestrator tui`; `cargo clippy -p superorchestrator --all-targets -- -D warnings`.
- Out of scope: Sessions, Models and Settings content; Levels tab changes.

### Task 3: web Decider section

Goal: a "Decider" section on the web Settings page using `/api/v1/decider`.

- Fields: endpoint, model and fallback model (free text with the suggestions offered as a datalist or combobox), `on_unreachable`, timeout, `min_confidence`, `min_margin`. Remove `min_confidence`, `min_margin`, `key_env`, `key_file` from the generic ConfigForm so each is edited once.
- Key: choose env var, key file, or paste (password input, cleared after save). Show the key source and the masked key, never a value from the API.
- Env overrides: a notice per shadowed field with the env var name and an explanation that it wins over the config.
- "Test decider" button: shows complexity, level, confidence, model, latency, or the error.
- Show `restart_required`.
- Tests: vitest for the form (validation messages, PATCH body, paste cleared, masked display, override notice, test result and error); a Playwright case with MSW in the browser e2e suite that saves a model and runs Test decider.
- Acceptance: `pnpm -C web lint && pnpm -C web typecheck && pnpm -C web test && pnpm -C web e2e`.
- Out of scope: other Settings sections, TUI.

### Task 4: TUI Sessions and Models tabs

- Sessions tab: list sessions (newest first: id, started, run count, active runs, title or cwd); Enter opens the session's runs; Enter on a run opens the existing run detail view (events and transcript tail). Reuse the Runs tab detail code; do not copy it.
- Models tab: the model catalog per harness (model, efforts, free flag, last refresh time) and `r` to refresh through `so_supervisor::services::models::refresh_and_save`.
- Tests: key handling, snapshots at 80x24 and 120x40 for each tab, empty states.
- Acceptance: `cargo test -p superorchestrator tui`; `cargo clippy -p superorchestrator --all-targets -- -D warnings`.
- Out of scope: send message and fork from the TUI; Levels tab.

### Task 5: TUI Settings tab with the decider

- Settings tab: the decider fields from `decider_get`, editable in a form; key choice env, file or paste (masked input); env override notices; `t` runs Test decider and shows the result; read-only summary of the rest of the config (config path, ui bind, caps). All calls go through the RPC methods from Task 1; no validation or writes in the TUI. Without a supervisor, show the not-running message.
- Tests: key handling, form submit sends the right RPC params (fake link), error from the supervisor reaches the status line, snapshots at 80x24 and 120x40, paste field is masked in the snapshot.
- Acceptance: `cargo test -p superorchestrator tui`; `cargo clippy -p superorchestrator --all-targets -- -D warnings`.
- Out of scope: editing non-decider config in the TUI.

### Task 6: web parity and docs

- Web: nav label "Routing" becomes "Levels"; nav order matches the parity table; Dashboard and Accounts show cooldowns with their end time next to headroom where missing (reset times are already shown).
- Docs: `docs/guide/parity.md` with the feature map table (feature, web page, TUI tab, gap reason), linked from `web-ui.md` and `tui.md`; update `web-ui.md`, `tui.md`, `config.md` (decider section, key paste); SPEC sections for the web UI and TUI; ADR `0012-supervisor-writes-decider-key-file.md`; CHANGELOG entry.
- Acceptance: `pnpm -C web lint && pnpm -C web typecheck && pnpm -C web test`.
- Out of scope: code outside `web/` and `docs/`, SPEC, CHANGELOG.

## Order

Task 1 and Task 2 run in parallel (no shared files). Task 3 after Task 1. Task 4 after Task 2. Task 5 after Tasks 1 and 4. Task 6 last. Then a final code-quality review, `cargo test --workspace`, the real check on a scratch supervisor, screenshots, and the PR.
