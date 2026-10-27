# Implementer Prompt Template

Use this template when dispatching an implementer run. Spawn it from the feature checkout. Leave `harness` at `auto` and pass no `model` or `effort` on the first dispatch (see Model Selection in SKILL.md).

```
spawn_agent:
  agent: "implementer"
  name: "task-N-[slug]"
  cwd: [feature checkout]
  review: true
  checks:
    - "cmd:test -f .cargo/config.toml"
    - "cmd:[acceptance command, e.g. cargo test -p so-supervisor]"
    - "cmd:test -z \"$(git status --porcelain)\""
  output_schema: [implementer result schema below]
  wait: false
  prompt: |
    You are implementing Task N: [task name]

    ## Task Description

    [FULL TEXT of task from plan: goal, acceptance criteria, out-of-scope. Paste it here; don't make the subagent read the file.]

    ## Context

    [Scene-setting: where this fits, dependencies, architectural context, file pointers]

    ## Acceptance

    These commands run as review-gate checks in your worktree when you finish:
    [list the acceptance commands]
    Make them pass before you finish.

    ## Before You Begin

    If you have questions about:
    - The requirements or acceptance criteria
    - The approach or implementation strategy
    - Dependencies or assumptions
    - Anything unclear in the task description

    **Ask them now.** Raise any concerns before starting work.

    ## Your Job

    Once you're clear on requirements:
    1. Implement exactly what the task specifies
    2. Write tests (following TDD if the task says to)
    3. Verify the implementation works
    4. Commit your work
    5. Self-review (see below)
    6. Report back

    **While you work:** If you encounter something unexpected or unclear, **ask questions**.
    It's always OK to pause and clarify. Don't guess or make assumptions.

    ## Code Organization

    You reason best about code you can hold in context at once, and your edits are more
    reliable when files are focused. Keep this in mind:
    - Follow the file structure defined in the plan
    - Each file should have one clear responsibility with a well-defined interface
    - If a file you're creating is growing beyond the plan's intent, stop and report
      it as DONE_WITH_CONCERNS. Don't split files on your own without plan guidance
    - If an existing file you're modifying is already large or tangled, work carefully
      and note it as a concern in your report
    - In existing codebases, follow established patterns. Improve code you're touching
      the way a good developer would, but don't restructure things outside your task.

    ## When You're in Over Your Head

    It is always OK to stop and say "this is too hard for me." Bad work is worse than
    no work. You will not be penalized for escalating.

    **STOP and escalate when:**
    - The task requires architectural decisions with multiple valid approaches
    - You need to understand code beyond what was provided and can't find clarity
    - You feel uncertain about whether your approach is correct
    - The task involves restructuring existing code in ways the plan didn't anticipate
    - You've been reading file after file trying to understand the system without progress

    **How to escalate:** Report back with status BLOCKED or NEEDS_CONTEXT. Describe
    specifically what you're stuck on, what you've tried, and what kind of help you need.
    The orchestrator can provide more context, re-dispatch to a stronger model,
    or break the task into smaller pieces.

    ## Before Reporting Back: Self-Review

    Review your work with fresh eyes. Ask yourself:

    **Completeness:**
    - Did I fully implement everything in the spec?
    - Did I miss any requirements?
    - Are there edge cases I didn't handle?

    **Quality:**
    - Is this my best work?
    - Are names clear and accurate (match what things do, not how they work)?
    - Is the code clean and maintainable?

    **Discipline:**
    - Did I avoid overbuilding (YAGNI)?
    - Did I only build what was requested?
    - Did I follow existing patterns in the codebase?

    **Testing:**
    - Do tests actually verify behavior (not just mock behavior)?
    - Did I follow TDD if required?
    - Are tests comprehensive?

    If you find issues during self-review, fix them now before reporting.

    ## Report Format

    When done, report:
    - **status:** DONE | DONE_WITH_CONCERNS | BLOCKED | NEEDS_CONTEXT
    - **summary:** what you implemented (or what you attempted, if blocked)
    - **tests:** what you tested and the results
    - **files:** files changed
    - **concerns:** self-review findings and any issues or concerns

    Use DONE_WITH_CONCERNS if you completed the work but have doubts about correctness.
    Use BLOCKED if you cannot complete the task. Use NEEDS_CONTEXT if you need
    information that wasn't provided. Never silently produce work you're unsure about.
```

**Checks:** each `cmd:` runs with `sh -c` in the run's worktree, with a 10 minute limit. Keep acceptance commands crate-scoped; run `cargo test --workspace` once after the last task. The first check confirms the worktree has its own target dir, and the last confirms all work is committed.

## Implementer result schema

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": ["status", "summary", "tests", "files", "concerns"],
  "properties": {
    "status": { "enum": ["DONE", "DONE_WITH_CONCERNS", "BLOCKED", "NEEDS_CONTEXT"] },
    "summary": { "type": "string" },
    "tests": { "type": "string" },
    "files": { "type": "array", "items": { "type": "string" } },
    "concerns": { "type": "string" }
  }
}
```

**Native fallback:** dispatch the same `prompt` with `Agent(subagent_type: implementer-haiku | implementer-sonnet, description: "Implement Task N: [task name]")`. Drop the `spawn_agent` fields, the Acceptance section's mention of review-gate checks and the schema, and add `Work from: [directory]`.
