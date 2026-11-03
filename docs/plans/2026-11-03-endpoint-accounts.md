# Epic E3: endpoint accounts — implementation plan

Design: [ADR 0008](../adr/0008-endpoint-accounts.md). Branch: `claude/endpoint-accounts-b80eca`.

Every task: TDD (failing test first), `cargo fmt`, `cargo clippy -p <crate> --all-targets -- -D warnings` clean, commit. Do not touch files outside the listed area without asking. Use CONTEXT.md terms; new term: **Endpoint account**.

Order: T1, then T2, T3 and T4 in parallel, then T5, then T6 and T7 in parallel, then T8.

## T1 — Endpoint model in so-core

Goal: `AccountFile` gains `endpoint: Option<Endpoint>` (TOML `[endpoint]`), with a preset table and validation.

- `Endpoint { preset: EndpointPreset, base_url: Option<String>, token_env: Option<String>, token_file: Option<String>, token_var: Option<String>, models: Vec<String>, slots: Slots { opus, sonnet, haiku: Option<String> } }`; `EndpointPreset` = `zai | kimi | minimax | custom` (lowercase serde).
- Preset defaults (const table): zai base `https://api.z.ai/api/anthropic`, token var `ANTHROPIC_AUTH_TOKEN`, slots opus/sonnet `glm-5.3`, haiku `glm-5.3-flash`, extra env `API_TIMEOUT_MS=3000000`, `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1`. kimi base `https://api.kimi.ai/coding/`, token var `ANTHROPIC_API_KEY`, all slots `k3.1-pro`. minimax base `https://api.minimax.io/anthropic`, token var `ANTHROPIC_AUTH_TOKEN`, all slots `MiniMax-M3.1-Flash`. custom: none.
- Resolved accessors: `base_url()`, `token_var()`, `slot(opus|sonnet|haiku)`, `extra_env()`.
- Validation in `AccountFile::validate`: endpoint only on `harness = "claude"`; exactly one of `token_env`/`token_file`; `token_env` a valid env name; `token_var` one of `ANTHROPIC_AUTH_TOKEN`/`ANTHROPIC_API_KEY`; `custom` needs `base_url`, non-empty `models` and all three slots; `base_url` starts with `https://` or `http://127.0.0.1`/`http://localhost`.
- `Paths::keys_dir()` = `<config dir>/keys`; `so_core::fsutil` helper `write_key_file(path, token)` creating the dir 0700 and the file 0600 atomically.
- `redact.rs`: redact values after `ANTHROPIC_AUTH_TOKEN=`, `ANTHROPIC_API_KEY=` and JSON `"ANTHROPIC_AUTH_TOKEN": "..."` / `"ANTHROPIC_API_KEY": "..."`, and the Z.ai key shape (32 hex chars, `.`, 16 alphanumerics).

Acceptance: `cargo test -p so-core`; `cargo clippy -p so-core --all-targets -- -D warnings`.
Tests: TOML round-trip with and without endpoint (old files load unchanged); every validation rule; key file mode 0600 and dir 0700; redaction cases.
Out of scope: launch env, routing, CLI, UI.

## T2 — Launch env, sign-in and CLI add

Goal: endpoint accounts launch with the right env everywhere and can be added from the CLI.

- `so-accounts/src/env.rs`: `build_launch`/`build_launch_vars` add `ANTHROPIC_BASE_URL`, `<token_var>=<token>`, `ANTHROPIC_DEFAULT_{OPUS,SONNET,HAIKU}_MODEL`, `ANTHROPIC_MODEL` = opus slot, and preset extra env. Token from `token_env` (read through `HostEnv`, add an env lookup there so tests inject it) or `token_file` (expand `~`, refuse if mode is not 0600, trim trailing newline). Missing token: a typed error naming the account and source, never the value.
- `Launch` gains `env_remove: Vec<String>` holding the other key variable (`ANTHROPIC_API_KEY` or `ANTHROPIC_AUTH_TOKEN`); `superorchestrator exec` applies it with `env_remove`. `Launch`'s `Debug` must not print token values (custom `Debug` that prints `[REDACTED]` for the token var).
- `signin.rs`: endpoint account is `SignedIn` when the token resolves, else `SignedOut`.
- `manage.rs`: `add_endpoint(paths, cfg, host, name, endpoint, token: Option<&str>)` creates the credential home like `add`, writes the key file to `keys/<name>.key` when a token is given (sets `token_file`), saves the account and wrapper.
- CLI `account add <name> --endpoint <preset> [--base-url URL] [--token-env VAR | --token-stdin] [--model ID ...] [--opus ID --sonnet ID --haiku ID]`; harness implied `claude`. `--token-stdin` reads one line from stdin. `account list/show` shows preset, base URL and token source kind, never the token.

Acceptance: `cargo test -p so-accounts`; `cargo test -p superorchestrator --bins cmd::` (or the crate's account CLI tests); clippy for both.
Tests: env vars for each preset; env for a custom endpoint; `env_remove`; `format!("{:?}", launch)` and `so_core::redact::redact` of the joined vars contain no token; plain claude account env unchanged; wrapper path (`exec`) uses the same vars (unit-test the function that builds the command); join command unchanged and resolves through the wrapper; key file written 0600; token-file with mode 0644 refused; CLI add round-trip.
Out of scope: routing, error classification, validation call, web and TUI.

## T3 — Catalog and routing eligibility

Goal: endpoint models route only to accounts that serve them; endpoint choices are not demoted by the Claude rank.

- `so-router/src/catalog.rs`: claude model entries accept `endpoint = "<preset>"`; accessor `catalog.endpoint(harness, model) -> Option<EndpointPreset>`.
- `catalog/default.toml`: add `[[claude.models]]` with `endpoint`: `glm-5.3` (high), `glm-5.3-flash` (fast) for `zai`; `k3.1-pro` (high), `k3.1-pro-highspeed` (mid), `k3` (mid), `kimi-for-coding` (mid) for `kimi`; `MiniMax-M3.1-Flash` (mid) for `minimax`. `cost = "included"`, no `efforts`.
- `eligible.rs`: a `serves(account, choice, catalog)` rule per ADR 0008 decision 3, applied in `usable_accounts` (and anywhere an account is picked for a pinned choice or pinned account in `route.rs`/`select.rs`). `apply_claude_rank` demotes only choices with no catalog endpoint. Where `router.priority` orders harnesses, an endpoint choice ranks just ahead of `claude`.
- A pinned endpoint account with no model pinned uses its opus slot model.

Acceptance: `cargo test -p so-router`; clippy.
Tests: eligibility matrix — Anthropic model only to plain Claude accounts; `glm-5.3` only to zai accounts; `k3.1-pro` only to kimi; custom account with `models` gets only those; Claude rank keeps endpoint choices when other harnesses are eligible; pinned endpoint account.
Out of scope: launch env, UI, levels config defaults.

## T4 — Error classification and validation call

Goal: provider quota and rate-limit errors classify correctly; a cheap token validation exists.

- `so-harness/src/claude.rs` `classify_error`: Z.ai codes 1308, 1309, 1310, 1316-1321 and "limit exhausted" → `Quota`; 1302, 1305, 1312 → `RateLimit`; Kimi "5-hour usage limit", "weekly (7-day) usage limit", "monthly usage limit" → `Quota`, "engine is currently overloaded", "too many requests", "concurrent request limit" → `RateLimit`, "membership benefits" → `Auth`; MiniMax 2056 → `Quota`, 1002 → `RateLimit`, 2049 → `Auth`. Match codes as words in context (e.g. `"code":"1308"` or `1308`), not inside longer numbers. If the supervisor has a reset-time hook for Quota errors, parse Z.ai `Your limit will reset at YYYY-MM-DD HH:MM:SS` as UTC+8; otherwise skip.
- Validation: `so_harness::endpoint::validate(base_url, token_var, token, model) -> Result<(), ValidateError>` sends `POST {base_url}/v1/messages` (`max_tokens: 1`, one user message `ping`, header `anthropic-version: 2023-06-01`; `x-api-key` when token var is `ANTHROPIC_API_KEY`, else `Authorization: Bearer`), 20 s timeout; maps non-2xx through the classifier into Auth/Quota/RateLimit/Other with the redacted body.
- Tests in the normal suite run `validate` against a mock Anthropic-compatible axum server: 200 is ok, 401 is Auth, 429 with a Z.ai 1308 body is Quota, and the request carries the right auth header and path. An `#[ignore]` test `real_endpoint_smoke` reads `SO_SMOKE_ENDPOINT_PRESET` and `SO_SMOKE_ENDPOINT_TOKEN` and runs `validate` against the real preset.

Acceptance: `cargo test -p so-harness`; clippy.
Out of scope: wiring into `account login` (T5), UI.

## T5 — Supervisor API and login wiring

Goal: the web API and supervisor protocol can add endpoint accounts and the CLI login validates.

- Supervisor protocol / web handler for account add accept an optional endpoint (preset, base_url, models, slots, token or token_env). A token is written via `manage::add_endpoint` to a key file; responses and account listings expose preset, base URL, models and token source kind (`env` or `file`), never the token. Regenerate the web API types the way the repo does (`web/src/api/generated.ts`).
- `account login <endpoint account>` and the web/TUI sign-in action call `so_harness::endpoint::validate` with the haiku slot model and report the result.
- Headroom refresh skips endpoint accounts (no probe); `list_harnesses` shows them.

Acceptance: `cargo test -p so-web`; `cargo test -p so-supervisor`; `cargo test -p superorchestrator`; clippy.
Out of scope: web dialog and TUI screens.

## T6 — Web add-account dialog

Goal: `web/src/features/accounts/AddAccountDialog.tsx` supports endpoint accounts.

- With harness `claude`: an "Endpoint" select (None, Z.ai GLM, Kimi Code, MiniMax, Custom). Preset fills base URL (read-only unless Custom). Token as a password field, or a token env var name. Custom shows base URL, models and slot fields. Schema in `schemas.ts`. Accounts page shows the preset badge.

Acceptance: `pnpm -C web test`; `pnpm -C web lint`; `pnpm -C web build`.
Tests (vitest): preset fills base URL; custom requires base URL and models; submit payload carries token, never shows it after submit; validation errors.
Out of scope: backend.

## T7 — TUI add-account flow

Goal: the TUI add-account flow supports endpoint accounts.

- After choosing harness `claude`, an endpoint step (none, zai, kimi, minimax, custom); then a masked token input (or env var name); custom asks base URL and models. Submits through the same supervisor call as the web.

Acceptance: `cargo test -p superorchestrator tui::`; clippy.
Tests: `tui/tests.rs` style flow test plus insta snapshot of the endpoint step and masked token field.
Out of scope: web.

## T8 — Docs

Goal: SPEC, CONTEXT, guide pages.

- CONTEXT.md: **Endpoint account** term (and avoid-list). SPEC: account file `[endpoint]` (5.1), launch env, routing rule and Claude rank change (8.x), sign-in, key files. Guide: `docs/guide/endpoints.md` overview plus one section per provider (Z.ai, Kimi, MiniMax, custom) with setup command, models, quota notes, errors; link from `accounts.md` and `routing.md`.

Acceptance: `scripts` CI job commands for docs, if any (see `.github/workflows/ci.yml`).
Out of scope: code.
