# Accounts and wrappers

An account is one named login for one harness, such as `codex-acc2`. Superorchestrator routes, caps and meters work per account. Each account has a credential home, a wrapper command and a settings file.

## The account file

`~/.config/superorchestrator/accounts/<name>.toml`:

```toml
name = "codex-acc2"
harness = "codex"            # codex | agy | opencode | claude
home = "~/.codex-acc2"       # credential home
binary = "codex"             # resolved on PATH at launch; an absolute path is allowed
priority = 0                 # tie-breaker inside a harness, higher first
cap = 2                      # most concurrent runs, interactive sessions included
enabled = true
pools = ["default"]          # agy: ["gemini", "claude"]
```

A name matches `^[a-z][a-z0-9-]{1,31}$`. The name is also the wrapper command, so it must not be `codex`, `agy`, `opencode` or `claude`, and it must not shadow another command: `account add` and `account import` fail when the name already resolves on `PATH` to an executable that is not one of our wrappers (for example `ls`).

## Credential homes

The credential home holds the login and the harness state. It lives under `$HOME`, so you can sign in with the harness's own CLI. Superorchestrator links to it and never copies credentials.

| Harness | Default home | Selected by |
|---|---|---|
| codex | `~/.<name>` | `CODEX_HOME` |
| opencode | `~/.<name>` | `XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `XDG_STATE_HOME` under the home |
| claude | `~/.<name>` | `CLAUDE_CONFIG_DIR` |
| agy | `~/.<name>` holding `.gemini/` | `--gemini_dir` (see [ADR-0003](../adr/0003-agy-accounts-isolate-gemini-dir.md) and [ADR-0005](../adr/0005-agy-runs-use-per-run-gemini-dir.md)) |

## Wrappers

`account add` and `account import` write a wrapper at `~/.local/bin/<name>` (change the directory with `accounts.wrapper_dir`):

```sh
#!/bin/sh
exec superorchestrator exec codex-acc2 -- "$@"
```

The wrapper sets the account's environment, records an interactive session, and starts the real binary. Use it as you would the harness: `codex-acc2 resume <thread>`. If an unrelated file already has that name, it moves to `~/.config/superorchestrator/wrappers.bak/` first. Wrappers we write carry a marker comment. Put `~/.local/bin` on your `PATH`.

## Commands

| Command | What it does |
|---|---|
| `account add <harness> [--name <name>] [--count K] [--home PATH] [--login]` | Creates the home, the file and the wrapper. Without `--name`, names are `<harness>-<n>` with the lowest free `n`. |
| `account import <name> --harness H --home PATH` | Links an existing home |
| `account import --scan` | Finds existing homes and offers to import each (`--yes` imports all) |
| `account login <name>` | Runs the harness login with the account's environment |
| `account list` | Name, harness, signed in, enabled, cap, active runs, headroom |
| `account remove <name> [--purge]` | Removes the file and wrapper. `--purge` also deletes the home. |
| `account enable <name>`, `account disable <name>` | Switch routing on or off |
| `account flag <name> [--pool P] --hours N` | Manual cooldown. Also `--until <RFC 3339>` and `--clear`. |

`account add codex --count 3` creates `codex-1`, `codex-2` and `codex-3`; a later run fills the first free number. A number is skipped when the account exists or the name would shadow a command on `PATH`. `--name` names one account, so it cannot be combined with `--count` above 1. `--login` signs in to each new account in turn when stdin is a terminal, and prints the commands otherwise.

## Setup

`superorchestrator setup` does the first-run work in one guided pass: it creates the config, lists the installed harnesses, offers the logins it finds (`account import --scan`), asks how many accounts to add for each harness, signs in, runs `install` and prints a `doctor` summary. Run it again any time; it adds nothing unless you ask for more.

```
superorchestrator setup                       # prompts (needs a terminal)
superorchestrator setup --yes                 # one account for each harness that has none
superorchestrator setup --yes --codex 3 --opencode 2 --no-login
```

Claude accounts are kept for the orchestrator. `setup` adds none by default (`--claude N` adds them), and the `claude-sonnet` and `claude-haiku` choices stay disabled, so routing never spends them. To let routing use them as a last resort, run `superorchestrator config edit` and set `enabled = true` on those choices. Without a terminal and without `--yes`, `setup` prints what it would do and changes nothing.

`--scan` looks for `~/.codex`, `~/.codex-accN`, `~/.agent-farm/profiles/agy_worker_N/.gemini`, `~/.local/share/opencode` and `~/.claude`. `--scan` offers homes you have signed in to: a home counts only when it holds the harness's auth file (checked by existence, never read). Directories that `install` or a harness probe leave behind, such as `~/.claude/backups` or `~/.local/share/opencode/log`, are not offered. Claude may keep its login in the keychain, so `~/.claude` also counts when it has a `projects` directory.

## Logging in, per harness

| Harness | How to sign in |
|---|---|
| codex | `superorchestrator account login <name>`. With a terminal it runs `codex login`. Without one it runs `codex login --device-auth` and prints a URL and a code. |
| opencode | `superorchestrator account login <name>` runs `opencode auth login` with the account's environment. It needs a terminal. |
| claude | Run the wrapper (`<name>`), then type `/login` inside the session. |
| agy | Run the wrapper (`<name>`), then sign in from inside the agy session. |

The web UI shows the same steps on the Accounts page.

## Sign-in check

The check only tests that the harness's auth file exists:

- codex: `auth.json`
- claude: `.credentials.json`
- opencode: `auth.json` or `data/opencode/auth.json`
- agy: a file under `.gemini/` such as `antigravity-cli/antigravity-oauth-token`

For Claude on macOS, and for the default `~/.claude`, credentials may live in the system keyring. The state is then `unknown` and the account stays usable. Auth file contents are never read, logged or served.

## Caps and pools

`cap` limits how many runs one account has at once. The global cap is `limits.global_cap` (default 16). A spawn over a cap waits in a queue. agy accounts have two pools, `gemini` and `claude`, metered separately.

## macOS

agy multi-account is Linux only until it is checked on a Mac. On macOS one agy account may be enabled, and it uses the real `~/.gemini`. `doctor` reports this.
