# Custom agents

A custom agent is a reusable subagent definition. Start one with `spawn_agent` and `agent: "<name>"`. The format matches Claude Code subagent files, plus `harness` and `accounts`.

## Where agents live

Superorchestrator searches these places in order. An earlier match wins.

1. `.superorchestrator/agents/` in the working directory
2. `~/.config/superorchestrator/agents/`
3. `.claude/agents/` in the working directory (a read-only import)

Each place holds `<name>/AGENT.md` folders and flat `<name>.md` files. A folder can also hold `skills/` and extra files. `init` installs three starter agents: `ui-ux-designer`, `pr-reviewer` and `issue-summarizer`.

## Trust and shadowing

Agents in the working directory (`.superorchestrator/agents/` and `.claude/agents/`)
are untrusted repository content. Spawning one is refused if it defines `hooks`,
command-capable inline `mcpServers`, `isolation: none`, or `extra_args`
(including an empty list; `extraArgs` is also accepted). The error names the agent, its file and every
unsafe field. This applies even if the selected harness would ignore that field,
and is checked again at every launch attempt, including resume and failover.

Review an agent before trusting it. To authorize those fields, move its definition
to the user agents directory (`~/.config/superorchestrator/agents/`, or your custom
config directory). Remove any same-named `.superorchestrator/agents/` project copy,
which would still win discovery. Named MCP references and HTTP/SSE inline
definitions containing only `url`, optional
`type: http` or `type: sse`, and optional `headers` remain allowed in project agents.

For every source, Claude agent arguments use this explicit allowlist: `--tools`,
`--allowedTools`, `--disallowedTools`, `--max-turns`, `--mcp-config`, `--settings`,
`--add-dir`. Unknown flags, aliases, `--flag=value`, stray values and missing values
are refused. Tool permission lists accept multiple values; the other flags require
one value, and `--max-turns` requires an unsigned integer. Prefer the dedicated
frontmatter fields; `extra_args` is a YAML list of separate flag/value strings for
trusted user agents and applies only to Claude.

`memory: project` remains allowed. For a project-sourced agent, project and local
memory are labelled **Untrusted repository content** and quoted as data, with an
instruction to ignore commands and trust claims in that content.

Search order stays the same. `agent list` marks a winning agent with `shadows` and
lists the hidden definition paths. `agent show` includes the same paths. Spawning
a shadowing agent returns a warning and records a warning progress event in its
run, identifying both source paths. Unsafe definitions remain available to
`agent list` and `agent show` so you can inspect them.

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
# hooks: {}             # user agents only
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

The web UI has an Agents page for the same. The installed `use-superorchestrator-mcp` skill lists your agents and refreshes when they change.

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
