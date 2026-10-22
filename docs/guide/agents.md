# Custom agents

A custom agent is a reusable subagent definition. Start one with `spawn_agent` and `agent: "<name>"`. The format matches Claude Code subagent files, plus `harness` and `accounts`.

## Where agents live

Superorchestrator searches these places in order. An earlier match wins.

1. `.superorchestrator/agents/` in the working directory
2. `~/.config/superorchestrator/agents/`
3. `.claude/agents/` in the working directory (a read-only import)

Each place holds `<name>/AGENT.md` folders and flat `<name>.md` files. A folder can also hold `skills/` and extra files. `init` installs three starter agents: `ui-ux-designer`, `pr-reviewer` and `issue-summarizer`.

## File format

`AGENT.md` has YAML frontmatter, then the system prompt.

```yaml
---
name: pr-reviewer
description: Reviews a PR diff for correctness bugs.
harness: auto            # or codex | agy | opencode | claude
accounts: [codex-acc2]   # optional allow-list
model: gpt-6.1-sol
effort: high
tools: [Read, Grep, Bash]
disallowedTools: [Write]
skills: [gh-cli]
mcpServers: [github]
hooks: {}
memory: project          # user | project | local
maxTurns: 40
isolation: worktree
review: false
---
System prompt body.
```

`description` is required. Every other field is optional, and the name defaults to the folder or file name. Model selection order: call argument, agent file, `defaults` in `config.toml`, router.

## Commands

| Command | What it does |
|---|---|
| `agent create <name> [--harness H] [--description D]` | Scaffolds a file in the config agents dir |
| `agent list` | Lists agents from all places |
| `agent show <name>` | Shows fields and the gaps for each harness |
| `agent edit <name>` | Opens the file in `$EDITOR`, then checks it |
| `agent remove <name>` | Deletes it (not for `.claude/agents` imports) |

The web UI has an Agents page for the same. The installed skill lists your agents and refreshes when they change.

## How an agent reaches each harness

At spawn, the agent is written in each harness's own format in the run directory.

- Codex: `config.toml` overrides, `AGENTS.md` and `skills/`.
- OpenCode: `opencode.json` agent and permission entries.
- Claude: `--mcp-config` and `--append-system-prompt-file`.
- agy: `--json-schema`, the system addendum, and skills linked into the run workspace.

## Per-harness gaps

Harnesses differ. Superorchestrator never fakes a control. `agent show` lists what each harness cannot honour.

| Field | Claude | Codex | OpenCode | agy |
|---|---|---|---|---|
| `tools`, `disallowedTools` | Entries with `(...)` or `mcp__` only auto-approve. They do not restrict. | Not supported. Codex has only sandbox and approval policy. | Mapped to permissions where possible | Not supported. Tool access is all or nothing. |
| `hooks` | Passed | Not passed | Not supported | Not supported |
| `skills` | Loaded | Loaded | Not loaded from the run directory | Linked into the run workspace |
| `mcpServers` by name | Resolved | Define inline instead | Define inline instead | Not configured from agent files |
| `mcpServers` inline | Passed | Passed | Passed | Not supported |

`memory` loads `MEMORY.md` from the chosen scope into the instructions on every harness.
