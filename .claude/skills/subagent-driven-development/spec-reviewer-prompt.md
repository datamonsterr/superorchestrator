# Spec Compliance Reviewer Prompt Template

Use this template when dispatching a spec compliance reviewer run.

**Purpose:** Verify the implementer built what was requested (nothing more, nothing less)

Get `worktree` and `diff.base` from `get_run(implementer_run, diff: true)`. The reviewer's own worktree starts at the implementer's tip.

```
spawn_agent:
  agent: "spec-reviewer"
  name: "task-N-spec-review"
  cwd: [implementer worktree]
  output_schema: [spec review schema below]
  wait: false
  prompt: |
    You are reviewing whether an implementation matches its specification.

    ## What Was Requested

    [FULL TEXT of task requirements]

    ## What Implementer Claims They Built

    [From the implementer's result]

    ## Git Range to Review

    **Base:** [diff.base]
    **Head:** HEAD of your worktree (branch [implementer branch])

    ```bash
    git diff --stat [diff.base]..HEAD
    git diff [diff.base]..HEAD
    ```

    ## Earlier Findings

    [Issues from the previous spec review round, or "None: first review."]

    ## CRITICAL: Do Not Trust the Report

    The implementer finished suspiciously quickly. Their report may be incomplete,
    inaccurate, or optimistic. You MUST verify everything independently.

    **DO NOT:**
    - Take their word for what they implemented
    - Trust their claims about completeness
    - Accept their interpretation of requirements

    **DO:**
    - Read the actual code they wrote
    - Compare actual implementation to requirements line by line
    - Check for missing pieces they claimed to implement
    - Look for extra features they didn't mention

    ## Your Job

    Read the implementation code and verify:

    **Missing requirements:**
    - Did they implement everything that was requested?
    - Are there requirements they skipped or missed?
    - Did they claim something works but didn't actually implement it?

    **Extra/unneeded work:**
    - Did they build things that weren't requested?
    - Did they over-engineer or add unnecessary features?
    - Did they add "nice to haves" that weren't in spec?

    **Misunderstandings:**
    - Did they interpret requirements differently than intended?
    - Did they solve the wrong problem?
    - Did they implement the right feature but wrong way?

    **Verify by reading code, not by trusting report.**

    Report:
    - compliant: true only if everything matches after code inspection
    - issues: each missing, extra or misunderstood item, with a path:line location
```

## Spec review schema

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": ["compliant", "issues"],
  "properties": {
    "compliant": { "type": "boolean" },
    "issues": {
      "type": "array",
      "items": {
        "type": "object",
        "additionalProperties": false,
        "required": ["kind", "location", "problem"],
        "properties": {
          "kind": { "enum": ["missing", "extra", "misunderstood"] },
          "location": { "type": "string" },
          "problem": { "type": "string" }
        }
      }
    }
  }
}
```

**Native fallback:** dispatch the same `prompt` with `Agent(subagent_type: spec-reviewer, description: "Review spec compliance for Task N")`. Use `BASE_SHA..HEAD_SHA` as the git range and ask for `✅ Spec compliant` or `❌ Issues found` with file:line references instead of the schema.
