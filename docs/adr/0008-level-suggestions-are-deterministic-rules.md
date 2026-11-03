# Level suggestions are deterministic rules

Status: accepted · 2026-11-03

## Context

Operators need a way to inspect how configured complexity levels perform and to find concrete edits. Routing history records outcomes, model refs, complexity, failover and escalation data. The current level configuration is the mapping used for analysis; historical level names may no longer match it.

Suggestions change routing behavior and must be reviewable and repeatable. An LLM could produce plausible edits, but its reasoning and output can vary, and it would be harder to validate each change.

## Decision

1. **Use deterministic rules.** Analyze history and catalog data locally. Each suggestion has a stable id, rationale, diff and complete proposed `levels`; reject it if `validate_levels` fails.
2. **Map history under current levels.** Assign each record by its complexity to the nearest current level. Outcomes use this precedence: rejected when any review was rejected, then accepted, failed, succeeded, otherwise other. Other is excluded from outcome rates. A bad outcome is rejected, failed, escalated or failed over.
3. **Require useful samples.** Rules that compare rates require at least five runs. This avoids acting on very small samples while keeping suggestions available with modest history.
4. **Apply five rules.**
   - `shift-boundary`: for at least five runs in the five-point band below a level's upper bound, shift that bound down five points when at least 50% have bad outcomes. Move the adjacent lower bound too when the ranges touch.
   - `promote-model`: when each compared model has at least five runs, move a later model before an earlier model if its success rate is at least 20 percentage points higher. Pick the strongest such improvement.
   - `assign-model`: append each unassigned catalog model to a level with a matching tier, choosing the highest matching level for high tier, lowest for fast, or middle for mid. Select the model's last listed effort when present.
   - `close-gap`: extend the first level to zero, a level to the next range, or the final level to 100 to cover a gap.
   - `remove-empty`: remove a level when none of its models exist in the catalog and transfer its range to an adjacent level.
5. **Apply through existing writes.** A suggestion carries the full `levels` value so it can be reviewed and validated as a complete configuration. CLI apply and web/TUI apply use the existing levels write path, including validation and the restart requirement; suggestions do not write config during analysis.

## Consequences

- The same history, catalog and levels produce the same ordered suggestions and ids.
- Operators can review exact proposed ranges and model order before applying them.
- Suggestions remain guidance: sample history and catalog completeness affect which rules can fire.
