---
name: init-omni-project
description: Bootstrap a project for use with the OmniAgent multi-agent orchestration system. Sets up .omnigent/config.yaml, AGENTS.md, GitHub PR workflow hooks, jev-skill-selector hooks for all agent harnesses (claude, codex, agy, opencode), and creates an initial PR skeleton. Use when the user says /init-omni-project or "set up omniagent for this project".
---

# /init-omni-project

Sets up a project directory for **OmniAgent** (Omnigent + Claude + Codex + Antigravity worker pool) orchestration.

## What It Does

1. Creates `.omnigent/config.yaml` — project-level Omnigent agent overrides
2. Creates `AGENTS.md` — agent instructions file (picked up by agy, claude, pi, etc.)
3. Installs jev-skill-selector hooks for all harnesses (`claude`, `codex`, `agy`, `opencode`)
4. Creates a `.github/PULL_REQUEST_TEMPLATE.md` for conductor-reviewed PRs
5. Creates `worktrees/` directory in parent folder (sibling to repo) for agy branch isolation
6. Opens a test session with `conductor-claude` on Omnigent to verify the setup

## Usage

Run in any git repository root:

```
/init-omni-project
```

Or trigger by saying: *"Initialize this project for omniagent"*

## Steps to Execute

### 1. Verify prerequisites
```bash
which omnigent agy claude codex opencode jev-skill-selector
systemctl --user is-active omni-agent.service
curl -s http://localhost:6767/v1/agents | jq -r '.[].name'
```

### 2. Create project-level Omnigent config
```bash
mkdir -p .omnigent
cat > .omnigent/config.yaml << 'EOF'
# Project-level Omnigent config
# Inherits global ~/.omnigent/config.yaml; overrides apply per-project.
# harness:
#   claude-sdk:
#     model: claude-sonnet-4-5
EOF
```

### 3. Create AGENTS.md (project instructions for all agents)
```bash
cat > AGENTS.md << 'EOF'
# Agent Instructions

This project is managed under the OmniAgent orchestration system.

## Architecture
- **Conductor**: Claude or Codex plans and reviews
- **Workers**: Antigravity (agy) pool executes tasks in isolated Git worktrees

## Workflow
1. Describe task to conductor-claude or conductor-codex in Omnigent chat
2. Conductor decomposes and delegates via `dispatch-agy "subtask"`
3. Worker creates branch `agy-<timestamp>-<random>`, implements, commits
4. Conductor reviews worktree diff, runs tests, approves merge

## Conventions
- Branch prefix: `agy-` for worker branches, `feat/` for conductor branches
- Always run tests before declaring a task complete
- Commit messages: Conventional Commits format
EOF
```

### 4. Install jev-skill-selector hooks
```bash
cd ~/dev/jev_skill_selector
bun run setup --status  # verify current state
bun run setup           # install/update all hooks
```

### 5. Create worktrees parent directory
```bash
# Sibling to the current repo
mkdir -p "$(git rev-parse --show-toplevel)/../worktrees"
echo "worktrees/" >> "$(git rev-parse --show-toplevel)/../.gitignore" 2>/dev/null || true
```

### 6. Add PR template
```bash
mkdir -p .github
cat > .github/PULL_REQUEST_TEMPLATE.md << 'EOF'
## Summary
<!-- What does this PR do? -->

## Agent
<!-- Which agent produced this branch? agy-worker-1/2/3 or manual? -->

## Changes
<!-- List key changes -->

## Tests
- [ ] Tests pass locally
- [ ] Conductor review completed in Omnigent chat
- [ ] No regressions in existing test suite

## Worktree Branch
<!-- Branch name from dispatch-agy output, e.g. agy-1234567890-42 -->
EOF
```

### 7. Verify Omnigent can see agents
```bash
curl -s http://localhost:6767/v1/agents 2>/dev/null | jq -r '.[].name' || \
  omnigent server status
```

### 8. Test a conductor session
Open the Omnigent web UI:
- **Tailnet**: https://datamonster.tailce7f83.ts.net:6767
- **Local**: http://localhost:6767

Start a new session with `conductor-claude` and type: *"Hello, confirm you can see the project at $(pwd)"*

## Notes
- The global Omnigent server runs as a systemd user service (`omni-agent.service`)
- Accessible on tailnet at `https://datamonster.tailce7f83.ts.net:6767`
- Dispatch workers: `dispatch-agy "task description"` (available globally in PATH)
- Auth required — first run sets admin password (visible in `journalctl --user -u omni-agent.service`)
