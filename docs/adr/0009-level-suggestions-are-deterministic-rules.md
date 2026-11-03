# Level suggestions are deterministic rules

Status: accepted · 2026-11-03

## Context

Operators need a way to inspect how configured complexity levels perform and to find concrete edits. Routing history records outcomes, model refs, complexity, failover and escalation data. The current level configuration is the mapping used for analysis; historical level names may no longer match it.

Suggestions change routing behavior and must be reviewable and repeatable. An LLM could produce plausible edits, but its reasoning and output can vary, and it would be harder to validate each change.

## Decision

1. **Use deterministic rules.** Analyze history and catalog data locally. Each suggestion has a stable id, rationale, diff and complete proposed `levels`; reject it if `validate_levels` fails.
2. **Map history under current levels.** Assign each record by its complexity to the nearest current level. Outcomes use this precedence: rejected when any review was rejected, then accepted, failed, succeeded, otherwise other. Other is excluded from outcome rates. A bad outcome is rejected, failed, escalated or failed over.
3. **Require useful samples.** Rules that compare rates require at least five runs. This avoids acting on very small samples while keeping suggestions available with modest history. A success rate is the share of counted runs (outcome not other) that are not bad. BAD_RATE is 50%, a clear majority of bad outcomes. PROMOTE_MARGIN is 20 percentage points, a gap larger than noise at small samples.
4. **Apply five rules.**
   - `shift-boundary`: for at least five runs in the five-point band below a level's upper bound, shift that bound down five points when at least 50% have bad outcomes. The level above (B) has its `min` moved down by five too, only when B.min is A.max + 1, so gaps are kept. Skip the rule when A.max - 5 would be below A.min.
   - `promote-model`: when each compared model has at least five runs, move a later model before an earlier model if its success rate is at least 20 percentage points higher. Pick the strongest such improvement.
   - `assign-model`: append an unassigned catalog model to a level with a matching tier, choosing the highest matching level for high tier, lowest for fast, or middle for mid. A level's tier is its `tier`, else the majority catalog tier of its listed models, with ties going to the higher tier. Tier-only levels (no models list) are skipped, since they already admit the model. Select the model's last listed effort when present. Candidates for one level are ranked by: harness already listed in some level first, then free catalog cost, then ref id. At most `MAX_ASSIGN_PER_LEVEL` (3) are emitted per level; the last one's rationale says how many more were cut. Without a cap, a catalog of 100+ unlisted models buries every other rule.
   - `close-gap`: extend the first level to zero, a level to the next range, or the final level to 100 to cover a gap.
   - `remove-empty`: needs at least two levels and a level that lists models, none of which the catalog knows. Remove it. Its range goes to the lower neighbour (its `max` extended), or to the next level up (its `min` extended) when it is the lowest.
5. **Apply through existing writes.** A suggestion carries the full `levels` value so it can be reviewed and validated as a complete configuration. CLI apply and web/TUI apply use the existing levels write path, including validation and the restart requirement; suggestions do not write config during analysis.

## Consequences

- The same history, catalog and levels produce the same ordered suggestions and ids.
- Operators can review exact proposed ranges and model order before applying them.
- Suggestions remain guidance: sample history and catalog completeness affect which rules can fire.
