# Code Quality Reviewer Prompt Template

Use this template when dispatching a code quality reviewer run.

**Purpose:** Verify implementation is well-built (clean, tested, maintainable)

**Only dispatch after spec compliance review passes.**

```
spawn_agent:
  agent: "code-quality-reviewer"
  name: "task-N-quality-review"
  cwd: [implementer worktree; for the final review, the feature checkout]
  output_schema: [code quality schema in ./code-reviewer-template.md]
  wait: false
  prompt: |
    [Brief from ./code-reviewer-template.md]

    DESCRIPTION: [task summary, from the implementer's result]
    PLAN_OR_REQUIREMENTS: [full text of Task N; for the final review, the plan's goal and task list]
    BASE: [diff.base from get_run(implementer_run, diff: true); for the final review, git merge-base main HEAD]
    HEAD: HEAD of your worktree
    EARLIER_FINDINGS: [issues from the previous quality review round, or "None: first review."]
```

**In addition to standard code quality concerns, the reviewer should check:**
- Does each file have one clear responsibility with a well-defined interface?
- Are units decomposed so they can be understood and tested independently?
- Is the implementation following the file structure from the plan?
- Did this implementation create new files that are already large, or significantly grow existing files? (Don't flag pre-existing file sizes — focus on what this change contributed.)

**Code reviewer returns:** approved, Strengths, Issues (critical/important/minor), Assessment

**Native fallback:** dispatch with `Agent(subagent_type: code-quality-reviewer)` and the same brief, with `BASE_SHA` and `HEAD_SHA` from this checkout. The reviewer returns the markdown format in `./code-reviewer-template.md` instead of the schema.
