# Config reference

Superorchestrator keeps its files in one directory:

```
~/.config/superorchestrator/
  config.toml        defaults, limits, router, usage, worktrees, ui, notifications, choices
  models.toml        model catalog
  accounts/<name>.toml
  agents/<name>/AGENT.md
  workflows/<name>.js
  state.db           SQLite, WAL mode
  runs/<run_id>/     transcript.jsonl, mcp.json, system.md, materialised agent files
  logs/              supervisor.log
  wrappers.bak/      wrappers that `account add` moved aside
  legacy-backup/<ts>/ backups from `install` and `migrate`
```

The directory is `$SUPERORCHESTRATOR_CONFIG_DIR` if set. Otherwise it is `$XDG_CONFIG_HOME/superorchestrator` on Linux, or `~/.config/superorchestrator`. Use `--config-dir DIR` for one command.

## Editing

```sh
superorchestrator config get limits.global_cap
superorchestrator config set limits.global_cap 8
superorchestrator config edit
```

`config set` keeps comments. Every section is optional, and an unknown key is an error. Durations are strings such as `"30m"`, `"5h"` or `"20s"`. The supervisor reads the file at start. The web UI tells you when a restart is needed.

## Keys

### `[limits]`

| Key | Default | Meaning |
|---|---|---|
| `global_cap` | `16` | Most active runs over all accounts. At least 1. |
| `max_children` | `8` | Most children one run may start. At least 1. |

### `[runs]`

| Key | Default | Meaning |
|---|---|---|
| `stall_ms` | `300000` | Time with no event before a run is `stalled` |
| `ask_timeout` | `"30m"` | How long `ask_parent` waits before it returns `timeout` |

### `[workflows]`

| Key | Default | Meaning |
|---|---|---|
| `max_duration` | `"6h"` | Longest one workflow run, waits included |

### `[usage]`

| Key | Default | Meaning |
|---|---|---|
| `probe_ttl` | `"5m"` | How old a usage reading may be before a new probe |
| `low_pct` | `15` | Headroom below this percent is `low` |
| `default_cooldown` | `"5h"` | Cooldown when an error gives no reset time |

### `[router]`

| Key | Default | Meaning |
|---|---|---|
| `priority` | `["agy", "opencode", "codex", "claude"]` | Harness order for ties |
| `weights` | `{ headroom = 1.0, load = 0.5, cost = 0.3 }` | Heuristic weights (`router.weights.headroom`, `.load`, `.cost`) |
| `instruction` | A cost-first instruction | Routing instruction sent to the decider |

### `[router.jev]`

| Key | Default | Meaning |
|---|---|---|
| `endpoint` | `https://openrouter.ai/api/alpha/decisions` | Decider URL |
| `model` | `~typesafe/jev-latest` | Decider model |
| `key_env` | `OPENROUTER_API_KEY` | Environment variable that holds the key |
| `key_file` | unset | Optional dotenv file that holds the key |
| `timeout` | `"20s"` | Decider timeout |

### `[router.policy]`

| Key | Default | Meaning |
|---|---|---|
| `min_confidence` | `0.6` | Lowest confidence to accept a decision |
| `min_margin` | `0.1` | Lowest gap between the top two probabilities |

The two defaults are placeholders until you run `route eval` against live Jev.

### `[worktrees]`

| Key | Default | Meaning |
|---|---|---|
| `dir` | `"{repo_parent}/{repo}-worktrees"` | Where run worktrees go. `{repo_parent}` and `{repo}` expand. |
| `branch_prefix` | `"so/"` | Prefix of run branches |

### `[accounts]`

| Key | Default | Meaning |
|---|---|---|
| `wrapper_dir` | `"~/.local/bin"` | Where wrappers are written |

### `[ui]`

| Key | Default | Meaning |
|---|---|---|
| `bind` | `"127.0.0.1"` | Address to bind. Anything but loopback needs a token. |
| `port` | `7878` | Port |
| `token` | unset | Shared secret for the UI. Plain text. The API cannot set it. |
| `enabled` | `false` | Start the UI whenever the supervisor starts. `ui --install-service` sets it. |

### `[supervisor]`

| Key | Default | Meaning |
|---|---|---|
| `idle_exit` | `"10m"` | The supervisor exits after this long with no runs, clients or UI streams |

### `[notifications]`

| Key | Default | Meaning |
|---|---|---|
| `discord_webhook_env` | `"SO_DISCORD_WEBHOOK"` | Name of the variable that holds the webhook URL |
| `desktop` | `true` | Desktop notifications |
| `on` | `["escalated", "question_waiting_10m", "session_finished", "account_exhausted"]` | Events that notify |

### `[defaults]`

Used when a spawn and its agent name no harness, model or effort.

| Key | Default | Meaning |
|---|---|---|
| `harness` | unset | `codex`, `agy`, `opencode` or `claude` |
| `model` | unset | Model id |
| `effort` | unset | Effort level |

### `[[choices]]`

See [routing](routing.md). Fields: `id`, `harness`, `model`, `effort`, `roles`, `description`, `enabled`.

## Environment variables

| Variable | Meaning |
|---|---|
| `SUPERORCHESTRATOR_CONFIG_DIR` | The config directory |
| `SUPERORCHESTRATOR_SOCKET` | The supervisor socket path |
| `SUPERORCHESTRATOR_RUN_ID` | Set for a subagent's MCP server, so it acts for that run |
| `SUPERORCHESTRATOR_LOG` | Log filter, for example `debug` |
| `SUPERORCHESTRATOR_SKIP_SKILL_REFRESH` | Any value turns off skill regeneration. Use it with a throwaway config dir against a real `HOME`. |
| `SUPERORCHESTRATOR_DOCTOR_TIMEOUT_MS` | `doctor` timeout for each `--version` call (default 5000) |
| `OPENROUTER_API_KEY` | The Jev key (name set by `router.jev.key_env`) |
| `SO_DISCORD_WEBHOOK` | The Discord webhook URL (name set by `notifications.discord_webhook_env`) |
| `SUPERORCHESTRATOR_INSTALL_DIR`, `SUPERORCHESTRATOR_RELEASE_URL` | Used by `install.sh` only |

The socket is `$XDG_RUNTIME_DIR/superorchestrator.sock` on Linux, with `/tmp/superorchestrator-<uid>/` as the fallback. On macOS it is `~/Library/Caches/superorchestrator/sock`.
