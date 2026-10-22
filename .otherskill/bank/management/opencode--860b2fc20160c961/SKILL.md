---
name: opencode
description: Expert guide for USING opencode CLI and web server. Covers non-interactive mode (opencode run), headless server (opencode serve), web interface (opencode web), attaching to servers, session management, model/provider config, and all CLI subcommands. Triggers on opencode commands, CLI usage, non-interactive AI coding, web server setup.
---

# OpenCode CLI & Web Server Guide

Load this skill when the user asks about using opencode as a tool — CLI commands, non-interactive mode, web server, sessions, models, or any `opencode <command>` usage.

## Installation

```bash
curl -fsSL https://opencode.ai/install | bash
# Or: npm install -g opencode-ai
# Or: brew install anomalyco/tap/opencode
```

## Global Options

All commands support: `--help`, `--version`, `--print-logs`, `--log-level <DEBUG|INFO|WARN|ERROR>`, `--pure` (disable plugins)

Global flags for TUI/server modes: `-m <provider/model>`, `-c` / `--continue` (resume last session), `-s <id>` / `--session` (resume specific session), `--fork` (copy on resume), `--agent <name>`, `--prompt <text>`

## Non-Interactive Mode: `opencode run`

Execute opencode without launching the TUI. Ideal for scripting, CI/CD, and automation.

```bash
# Basic usage
opencode run "Explain async/await in TypeScript"

# With specific model
opencode run -m anthropic/claude-sonnet-4-20250514 "Refactor this function"

# Attach files to the prompt
opencode run -f src/index.ts "Explain this file"

# Continue last session
opencode run -c "Now add error handling"

# Fork a session (creates copy)
opencode run -c --fork "Try a different approach"

# Output raw JSON events (for piping/processing)
opencode run --format json "List all TODO comments"

# Share the session after completion
opencode run --share "Generate a README"
```

### `run` Flags

| Flag | Description |
|------|-------------|
| `-m, --model <provider/model>` | Model to use |
| `--agent <name>` | Agent to use |
| `-c, --continue` | Continue last session |
| `-s, --session <id>` | Resume specific session |
| `--fork` | Fork session before continuing |
| `--share` | Share session after completion |
| `--format <default\|json>` | Output format (default: formatted) |
| `-f, --file <path>` | Attach file(s) to message (repeatable) |
| `--title <text>` | Title for the session |
| `--attach <url>` | Attach to running server (e.g., http://localhost:4096) |
| `-p, --password <pw>` | Basic auth password (env: OPENCODE_SERVER_PASSWORD) |
| `-u, --username <user>` | Basic auth username (default: opencode) |
| `--dir <path>` | Working directory |
| `--port <num>` | Local server port (random if omitted) |
| `--variant <level>` | Model reasoning effort (e.g., high, max, minimal) |
| `--thinking` | Show thinking blocks |
| `-i, --interactive` | Run in direct interactive split-footer mode |
| `--dangerously-skip-permissions` | Auto-approve non-denied permissions |
| `--command <cmd>` | Run a slash command instead of a message |

### Attaching `run` to a Server

Avoid MCP cold boot by attaching to a running server:

```bash
# Terminal 1: start server
opencode serve

# Terminal 2: run commands against it
opencode run --attach http://localhost:4096 "Fix the lint errors"
```

## Headless Server: `opencode serve`

Start a headless server without TUI. Other clients (run --attach, web, attach) connect to it.

```bash
# Start with defaults (random port, localhost)
opencode serve

# Specify port
opencode serve --port 4096

# Expose on all interfaces
opencode serve --hostname 0.0.0.0 --port 4096

# Enable mDNS discovery
opencode serve --mdns --port 4096

# Allow CORS for specific origins
opencode serve --cors http://localhost:5173 --port 4096
```

### `serve` Flags

| Flag | Default | Description |
|------|---------|-------------|
| `--port <num>` | 0 (random) | Port to listen on |
| `--hostname <addr>` | 127.0.0.1 | Bind address |
| `--mdns` | false | Enable mDNS service discovery |
| `--mdns-domain <name>` | opencode.local | Custom mDNS domain |
| `--cors <origin>` | [] | Additional CORS domains (repeatable) |

### Server REST API

The server exposes a REST API at the bound address. Key endpoints:

- `GET /app` — Application details
- `POST /app/init` — Initialize application
- `GET /config/providers` — List configured providers
- `GET /mode` — Available modes
- `GET /session` — List sessions
- `POST /session/init` — Initialize session
- `POST /session/summarize` — Summarize session
- `POST /session/abort` — Abort running session
- `DELETE /session/delete` — Delete session
- `GET /file/read?path=<path>` — Read file content
- `GET /file/status` — Modified files status
- `GET /find/file` — Search files
- `GET /find/symbol` — Search symbols
- `GET /find` — Search text

Authentication: Use `-u` / `-p` flags or `OPENCODE_SERVER_USERNAME` / `OPENCODE_SERVER_PASSWORD` env vars.

## Web Interface: `opencode web`

Start server and open the web UI in a browser. Same flags as `serve`.

```bash
opencode web
opencode web --port 4096 --hostname 0.0.0.0
opencode web --cors http://localhost:5173
```

You can attach a TUI to the same web server:

```bash
# Terminal 1
opencode web --port 4096

# Terminal 2
opencode attach http://localhost:4096
```

## Attach to Server: `opencode attach`

Connect a TUI to a running `serve` or `web` instance.

```bash
opencode attach http://localhost:4096

# With session resume
opencode attach http://localhost:4096 -c

# With specific session
opencode attach http://localhost:4096 -s <session-id>

# With auth
opencode attach http://localhost:4096 -u opencode -p secretpassword
```

| Flag | Description |
|------|-------------|
| `--dir <path>` | Working directory |
| `-c, --continue` | Continue last session |
| `-s, --session <id>` | Resume specific session |
| `--fork` | Fork on resume |
| `-p, --password <pw>` | Auth password |
| `-u, --username <user>` | Auth username |

## ACP Server: `opencode acp`

Start an Agent Client Protocol server (same flags as `serve` plus `--cwd`).

```bash
opencode acp --port 4096 --cwd /path/to/project
```

## Session Management

```bash
# List sessions
opencode session list
opencode session list -n 10 --format json

# Delete a session
opencode session delete <session-id>

# Export session as JSON
opencode export <session-id>
opencode export <session-id> --sanitize  # redact sensitive data

# Import session from file or share URL
opencode import session.json
opencode import https://opencode.ai/share/abc123
```

## Models & Providers

```bash
# List all available models
opencode models

# Filter by provider
opencode models anthropic

# Verbose (includes cost info)
opencode models --verbose

# Refresh model cache
opencode models --refresh

# Manage providers
opencode providers list
opencode providers login
opencode providers logout
```

## PR Workflow: `opencode pr`

Fetch a GitHub PR branch, checkout, and open opencode:

```bash
opencode pr 42
```

## GitHub Agent

```bash
opencode github install        # Install GitHub agent
opencode github run            # Run the agent
opencode github run --event '{"type":"issues","action":"opened",...}'
opencode github run --token github_pat_...
```

## MCP Servers

```bash
opencode mcp add               # Add an MCP server
opencode mcp list              # List servers and status
opencode mcp auth [name]       # Authenticate OAuth MCP server
opencode mcp logout [name]     # Remove OAuth credentials
opencode mcp debug <name>      # Debug OAuth connection
```

## Agent Management

```bash
# List agents
opencode agent list

# Create agent
opencode agent create --description "Review code" --mode subagent --tools "read,grep,glob"

# Create with model
opencode agent create -m anthropic/claude-sonnet-4-20250514 --mode primary

# Create at specific path
opencode agent create --path .opencode/agents/reviewer.md
```

| `create` Flag | Description |
|---------------|-------------|
| `--path <dir>` | Output directory |
| `--description <text>` | What the agent does |
| `--mode <all\|primary\|subagent>` | Agent mode |
| `--tools <list>` | Comma-separated permissions: bash,read,edit,glob,grep,webfetch,task,todowrite,websearch,lsp,skill |
| `-m <model>` | Model in provider/model format |

## Stats & Debugging

```bash
# Token usage and costs
opencode stats
opencode stats --days 7 --models --project myproject
opencode stats --tools 10

# Debug tools
opencode debug config           # Show resolved config
opencode debug paths            # Global paths (data, config, cache, state)
opencode debug info             # Debug information
opencode debug skill            # List available skills
opencode debug agent <name>     # Show agent config details
opencode debug startup          # Startup timing
opencode debug lsp              # LSP debugging
opencode debug rg               # Ripgrep debugging
opencode debug file             # File system debugging
opencode debug scrap            # List all known projects
opencode debug snapshot         # Snapshot debugging
```

## Database Tools

```bash
opencode db                     # Interactive SQLite shell
opencode db "SELECT * FROM session LIMIT 5"  # Run query
opencode db path                # Print database path
opencode db migrate             # Migrate JSON data to SQLite
opencode db --format json "SELECT count(*) FROM message"
```

## Plugins & Extensions

```bash
# Install plugin
opencode plugin <npm-module>
opencode plugin <npm-module> --global
opencode plugin <npm-module> --force  # Replace existing version
```

## Upgrade & Uninstall

```bash
# Upgrade
opencode upgrade
opencode upgrade 0.1.48
opencode upgrade --method curl

# Uninstall
opencode uninstall --dry-run       # Preview what gets removed
opencode uninstall -c -d -f        # Remove but keep config and data
```

## Shell Completion

```bash
opencode completion  # Generate completion script (bash/zsh/fish)
```

## Configuration

### Config Files

- **Project**: `.opencode/opencode.json`
- **Global**: `~/.config/opencode/config.json`

### Server Config (opencode.json)

```json
{
  "$schema": "https://opencode.ai/config.json",
  "server": {
    "port": 4096,
    "hostname": "0.0.0.0",
    "mdns": true,
    "mdnsDomain": "myproject.local",
    "cors": ["http://localhost:5173"]
  }
}
```

### Environment Variables

| Variable | Description |
|----------|-------------|
| `OPENCODE_AUTO_SHARE` | Auto-share sessions |
| `OPENCODE_CONFIG` | Custom config path |
| `OPENCODE_DISABLE_CLAUDE_CODE` | Disable Claude Code compat |
| `OPENCODE_SERVER_PASSWORD` | Default auth password |
| `OPENCODE_SERVER_USERNAME` | Default auth username |

## TUI Quick Reference

When using `opencode` interactively (default, no subcommand):

| Key/Command | Action |
|-------------|--------|
| `Tab` | Switch Build/Plan mode |
| `Ctrl+X` | Leader key for shortcuts |
| `/help` | Show help |
| `/init` | Create AGENTS.md |
| `/connect` | Configure AI provider |
| `/new` | New session |
| `/sessions` | List sessions |
| `/models` | Switch model |
| `/undo` / `/redo` | Git-based undo/redo |
| `/share` / `/unshare` | Share/unshare session |
| `/compact` | Compact context |
| `/details` | Show details |
| `/thinking` | Toggle thinking |
| `/export` | Export session |
| `@file` | Fuzzy file reference |
| `!command` | Execute shell command |
