---
name: jev-skill-selector
description: Fast, calibrated multi-skill router using TypeSafe Jev decision model on OpenRouter (~typesafe/jev-latest). Dynamically filters and injects only relevant skills from your skills bank per prompt to eliminate context bloat. Use on every prompt or before agent task execution.
---

# Jev Skill Selector

High-speed, calibrated multi-skill router powered by the **TypeSafe Jev System One** decision model on OpenRouter (`~typesafe/jev-latest`).

Instead of polluting the agent's context window by loading 70+ skills on startup, Jev evaluates the user prompt against your centralized skill bank in a single parallel tensor pass (~400–700ms) and returns only the necessary skills for that specific task.

## Quick Start

### 1. Run Jev and Get Full Skill Content (When Cannot Use Hooks)
If your agent harness cannot use lifecycle hooks, invoke `jev-skill-selector` CLI (globally available via `bun link` or via `./run.sh`) to select skills and output their complete `SKILL.md` instructions:

```bash
# If linked globally via 'bun link' (recommended):
jev-skill-selector "Refactor the payment gateway with TDD and make a git commit" --content

# Or using the skill runner script:
./run.sh "Refactor the payment gateway with TDD and make a git commit" --content

# Or directly with bun:
bun run /path/to/jev_skill_selector/index.ts "Refactor the payment gateway with TDD and make a git commit" --content
```

Directly inspect a skill by name:
```bash
jev-skill-selector --skill tdd
```

### 2. Fast Skill Selection Overview
To inspect routing probabilities without full file dumps:

```bash
jev-skill-selector "Refactor the payment gateway with TDD and make a git commit"
```

Output:
```text
🎯 Jev Skill Selection Result
Prompt: "Refactor the payment gateway with TDD and make a git commit"
Primary Choice: tdd
Confidence: 0.88

Selected Skills:
  • tdd                            Prob: 65.0% | Conf: 88.0%
  • git-commit                     Prob: 32.0% | Conf: 88.0%

📊 Token Usage Breakdown:
  • Total Used Tokens:    3977
  • Input Prompt Tokens:  14
  • System Prompt Tokens: 37
  • API Input Tokens:     3209
  • Output Tokens:        768
```

### 3. Programmatic TypeScript API:

```typescript
import { selectSkills, formatSkillContent } from "./index";

const result = await selectSkills({
  userPrompt: "Diagnose database connection leaks and write regression tests",
  options: { maxSkills: 3, threshold: 0.05, includeContent: true },
});

console.log(result.selectedSkills);
// => [ { name: "diagnosing-bugs", probability: 0.61, content: "..." }, { name: "tdd", probability: 0.38, content: "..." } ]

// Format full instructions for prompt injection
const injection = formatSkillContent(result.selectedSkills, {
  userPrompt: "Diagnose database connection leaks and write regression tests",
});
```

---

## Workflows

### 1. Pre-Prompt Hook Workflow (Priority 1: Automatic Routing)

The tool acts as a gatekeeper before your agent starts planning:
1. User submits a prompt (in Claude Code, Codex, Antigravity, or OpenCode).
2. The hook intercepts the prompt via stdin or lifecycle event.
3. Jev evaluates candidate skills against the prompt (~400–700ms).
4. Hook outputs the matching skill instructions (`SKILL.md` contents) directly into the agent's context for that turn.

### 2. Standalone Skill Workflow (Priority 2: Fallback When Cannot Use Hooks)

When the agent harness does not support lifecycle hooks:
1. Only `jev-skill-selector` is installed into the harness's skill folder (`~/.agents/skills/jev-skill-selector`).
2. The agent calls:
   ```bash
   jev-skill-selector "<task description>" --content
   # or using the skill's runner:
   ./run.sh "<task description>" --content
   ```
3. The full instructions of only the necessary skills are returned directly for execution.

### 3. Multi-Step & Continuation Tasks

For complex tasks (e.g. "Phase 1: investigate, Phase 2: test, Phase 3: deploy"), Jev automatically activates parallel decision questions:
- `primary_skill`: Core/initial action
- `secondary_skill`: Subsequent implementation
- `followup_skill`: Verification, documentation, git, or deployment

---

## Skills Directory Precedence

Skills are automatically discovered and merged from two standard locations:
1. **Agent Skills (Project-Level)**: `./.agents/jev_skills` (or `AGENT_SKILLS_PATH`)
2. **Global Skills (User-Level)**: `~/.agents/jev_skills/` (or `GLOBAL_SKILLS_PATH`)
3. **Precedence**: Project-level skills override global skills with the same name.
4. **Custom Bank**: Set `SKILLS_BANK_PATH` to specify a custom bank.

---

## Environment Configuration

Configure via environment variables or a `.env` file:

```env
# Required: OpenRouter API Key
OPENROUTER_API_KEY=sk-or-v1-your-key-here
# Optional alias
# JEV_API_KEY=sk-or-v1-your-key-here

# Optional: Jev model slug (defaults to ~typesafe/jev-latest)
MODEL=~typesafe/jev-latest

# Optional: Global skills bank directory (defaults to ~/.agents/jev_skills/)
GLOBAL_SKILLS_PATH=~/.agents/jev_skills

# Optional: Project/agent skills directory (defaults to ./.agents/jev_skills)
AGENT_SKILLS_PATH=./.agents/jev_skills
```

---

## Bundled Hooks & Tools

- **Codex Hook**: [`hooks/codex.ts`](hooks/codex.ts) - Injects `UserPromptSubmitCommandOutputWire` with full skill instructions.
- **Claude Code Hook**: [`hooks/claude.ts`](hooks/claude.ts) - Intercepts `UserPromptSubmit` in Claude settings and injects full skill instructions.
- **OpenCode Plugin**: [`plugins/opencode.ts`](plugins/opencode.ts) - Intercepts `messages.transform` and provides `select_skill` tool.
- **Antigravity Hook**: [`hooks/antigravity.ts`](hooks/antigravity.ts) - Pre-invocation ephemeral message with skill instructions.
- **Automated Setup Script**: [`scripts/backup-and-setup.sh`](scripts/backup-and-setup.sh) & [`scripts/backup-and-setup.ps1`](scripts/backup-and-setup.ps1).
