---
name: superorchestrator-cli
description: Drive the superorchestrator CLI for accounts, levels, config, models, agent profiles, workflows and runs.
---

# superorchestrator CLI

Prefix commands below with `superorchestrator`. Use `--help` for flags; globals: `--json`, `--yes`, `--config-dir DIR`.
If the shell lacks it, export `XDG_RUNTIME_DIR=/run/user/$(id -u)` so the CLI finds the supervisor socket.
Delegate through `use-superorchestrator-mcp`; edit setup through `superorchestrator-config`.

| Status / setup | Command |
|---|---|
| Headroom / diagnose | `status`; `doctor` |
| First setup / seed config | `setup`; `init` |
| Ship MCP + three skills | `install --dry-run`; `install`; `uninstall --dry-run` |
| Interfaces | `ui --no-open`; `tui` (also the default command) |

| Accounts | Command |
|---|---|
| Add / sign in | `account add HARNESS --name NAME --login`; `account login NAME` |
| Discover / import | `account list`; `account import NAME --harness H --home DIR`; `account import --scan` |
| Availability | `account enable NAME`; `account disable NAME`; `account flag NAME --hours N`; `account flag NAME --clear` |
| Remove | `account remove NAME` (`--purge` deletes its credential home) |

| Levels (model refs are `harness/model[/effort]`) | Command |
|---|---|
| Inspect / add | `levels list`; `levels add NAME --min N --max N --model REF` |
| Change / remove | `levels edit NAME --min N --max N`; `levels remove NAME`; `levels reset` |
| Add / order models | `levels model add LEVEL REF --position N`; `levels model move LEVEL REF --position N` |
| Remove model | `levels model remove LEVEL REF` |

| Config | Command |
|---|---|
| Read / write / edit | `config get KEY`; `config set KEY VALUE`; `config edit` |
| Preview routing | `route "TASK" --agent NAME --explain` |

| Models | Command |
|---|---|
| Catalog | `models list`; `models refresh` |

| Agent profiles | Command |
|---|---|
| Inspect | `agent list`; `agent show NAME` |
| Configure | `agent create NAME --description TEXT`; `agent edit NAME`; `agent remove NAME` |

| Workflows | Command |
|---|---|
| Start saved / file | `workflow run NAME --args JSON --detach`; `workflow run --file FILE --fresh` |
| Inspect / stop | `workflow list`; `workflow status ID`; `workflow cancel ID` |

| Runs | Command |
|---|---|
| Inspect / cancel | `runs list --all`; `runs show RUN --diff`; `runs cancel RUN` |
| Follow | `runs watch RUN`; `runs watch --workflow ID` |
| Attach to native session | `join RUN`; `join RUN --exec` |
| Worktrees | `worktrees list`; `worktrees prune` |
