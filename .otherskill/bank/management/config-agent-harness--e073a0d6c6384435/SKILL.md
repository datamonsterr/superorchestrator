---
name: config-agent-harness
description: Synchronize and configure multi-agent harnesses (Antigravity, OpenCode, Claude Code, Codex, Cursor, Gemini) all at once. Use when onboarding a new agent, syncing skills or rules across environments, updating subagents, or refreshing project-wide MCP configurations.
---

# Config Agent Harness

This skill standardizes and synchronizes customizations across all supported AI coding harnesses from a single unified source of truth.

## Source of Truth Architecture

- **Skills**: `.agents/skills/` (shared with `.claude/skills`, `.codex/skills`, `.opencode/skills`, `.cursor/skills` via symlinks).
- **Rules**: `.agents/rules/` (shared with Cursor via `.cursor/rules` and master `AGENTS.md`).
- **Instructions**: `AGENTS.md` (mirrored by `CLAUDE.md`, `CODEX.md`, `GEMINI.md`, `.cursorrules`).
- **Subagents**: Synchronized into `opencode.json` and agent definitions.
- **MCP Servers**: `mcp_config.json` mirrored to `.agents/mcp_config.json`, `opencode.json`, `.cursor/mcp.json`, and `.mcp.json`.
- **AI Logging**: Hooks configured for Antigravity, OpenCode, Claude Code, and Codex.

## Execution

To synchronize all harnesses in one command:

```bash
python3 scripts/sync_agent_harness.py
```

## Verification Steps

1. Verify skill symlinks resolve:
   ```bash
   ls -la .claude/skills .opencode/skills .codex/skills
   ```
2. Check MCP configurations:
   ```bash
   cat mcp_config.json
   ```
3. Test AI logging hooks:
   ```bash
   bash scripts/setup_hooks.sh
   ```
