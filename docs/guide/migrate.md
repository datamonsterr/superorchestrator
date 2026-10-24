# Migrating from the old tooling

`superorchestrator migrate` replaces the older local tooling: the `orchestrate` scripts and skill, `coding-agent-management`, `check-account-usage`, `add-agy-worker` and agent-dispatcher's `jev-router-mcp`. Skip this page if you never used them.

## Dry run first

```sh
superorchestrator migrate
```

This prints the plan and changes nothing. Read it. Then apply it:

```sh
superorchestrator migrate --apply
```

`--apply` shows the plan again and asks you to confirm. Pass `--yes` to skip the question. Before it changes anything, it checks that the plan is still the same. If your system changed in between, it stops and shows the new plan.

## What it does

1. Backs up everything it touches to `~/.config/superorchestrator/legacy-backup/<timestamp>/`, mirroring the original paths.
2. Imports the existing accounts by linking their homes, then rewrites their wrappers:
   - `codex-acc1` at `~/.codex`
   - `codex-acc2` to `codex-acc5` at `~/.codex-accN`
   - `agy-acc1` to `agy-acc5` at `~/.agent-farm/profiles/agy_worker_N/.gemini`
   - the OpenCode and Claude defaults
3. Imports cooldowns from `~/.local/state/coding-agent/.state/*/*.meta` as flags.
4. Removes `~/.claude/orchestrator/`, `~/.claude/orchestrator-state*.json`, `~/.claude/skills/coding-agent-management/`, `~/.local/bin/check-account-usage`, `~/.local/bin/add-agy-worker`, and the `jev-router-mcp` entries in the Codex, agy and OpenCode configs.
5. Rewrites the delegation rule in `~/.claude/CLAUDE.md`, so implementation goes through the superorchestrator tools and skill, not the `orchestrate` skill. Only the first line that names the old skill is replaced. Other such lines stay and are listed for you to edit. Lines with a prohibition ("never", "don't") are left alone. A diff is shown first.

If it finds any legacy artifact, it also runs `init` and `install`. On a clean machine it says so and points you to `superorchestrator install`.

## Safety

- Credential homes are never copied, and auth files are never opened.
- Claude Code's own `~/.claude.json` is not backed up, because `claude mcp add` edits it and it holds account data.
- A second `--apply` finds nothing to do and writes nothing.

## Environment variables

The old scripts ran harnesses in your shell environment. Supervisor-launched runs now start with an allowlisted environment, so API keys, `GH_TOKEN`, `SSH_AUTH_SOCK` and corporate CA variables are not inherited. If a migrated account needs one, list its name in `pass_env` (per account, or `accounts.pass_env` in `config.toml` for all), then restart the supervisor. `superorchestrator doctor` warns about common names that are set but not passed. See [accounts](accounts.md#inherited-environment).

## Undo

Copy files back from `legacy-backup/<timestamp>/`. Paths inside mirror the originals.
