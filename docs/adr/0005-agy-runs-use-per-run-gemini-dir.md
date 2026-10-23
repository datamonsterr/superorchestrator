# agy runs use a per-run gemini dir instead of a bind mount

Status: accepted · 2026-10-22 · amends [ADR-0003](0003-agy-accounts-isolate-gemini-dir.md)

The M3 spikes ([ADR-0004](0004-harness-driver-findings.md)) showed two things. agy accepts `--gemini_dir=<dir>` and keeps all account state there, so a bind mount over `~/.gemini` is not needed to select the account. And the agent-farm profiles symlink `config` to `~/.gemini/config`, so binding a profile over `~/.gemini` makes agy fail with `file exists`. agy also has no `--mcp-config` and reads MCP servers only from `<gemini_dir>/config/mcp_config.json`.

We therefore select an agy account with `--gemini_dir` alone and never bind it over `~/.gemini`:

- **Interactive launches** (wrapper `exec`): `agy --gemini_dir=<home>/.gemini …`. bwrap is not used.
- **Supervised runs**: the driver builds `<run_dir>/gemini/` with symlinks to the account's `antigravity-cli`, `plugins`, `projects` and `skills`, a real `config/` directory copied from the account, and a run-specific `config/mcp_config.json` that adds `superorchestrator mcp`. It launches `bwrap --die-with-parent --new-session --bind / / --dev-bind /dev /dev -- agy --gemini_dir=<run_dir>/gemini -p= --input-format stream-json --output-format stream-json …`. bwrap stays only for `--die-with-parent` and the new session. When bwrap is missing, the driver launches agy directly and kills it on supervisor exit.
- `HOME` is unchanged, as in ADR-0003.

The macOS single-account rule in SPEC 6.4 stays until it is checked on a Mac.

**Update (2026-10):** the per-run `config/` is copied from the account's `config`, which may be a symlink to a shared root. That root must be a directory owned by the current uid that is not world-writable; group write is accepted only when its group is the user's primary group. Top-level entries that are symlinks resolving outside the account and the config root are skipped with a warning, as dangling links are, rather than failing the launch.
