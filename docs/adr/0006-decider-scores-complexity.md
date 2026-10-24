# The decider scores complexity; user-defined levels pick the choice

Status: accepted · 2026-10-24

## Context

The Jev decider used to pick a choice itself (`questions.route`, `type: "choice"`). Its criteria were the eligible choices' descriptions, harness, model and effort, and its `state` carried quota remaining per model family and per-account usage, with account names. Every change to the config, such as a new description or a renamed account, changed its answers. The answer had to satisfy several exact checks: probabilities over exactly the eligible set, and a margin between the top two choices (`router.policy.min_margin`), which was tuned on a handful of live runs. Account names and quota were also sent to OpenRouter on every route.

The question we actually need answered is narrower: how much work does this task need? Several sources frame it as a scale. OpenRouter's Decisions API has a `score` question, where the criteria are an ordered list of anchors and the answer is a probability-weighted index. Hugging Face's `RowRed/ComplexityRouter` uses four ordinal levels. NVIDIA's `prompt-task-and-complexity-classifier` gives a 0 to 1 score from weighted dimensions such as reasoning and constraints. The SWE-bench Verified annotations bucket issues by the developer time the fix takes. `COMPLEXITY_ANCHORS` in `so-core` combines them into five anchors, from a trivial mechanical edit to open-ended architecture work, and the score is mapped linearly to 0 to 100.

## Decision

1. **The decider answers one question.** `questions.complexity` has `type: "score"`, the five anchors as `criteria`, and `router.instruction` as `instructions`. Its `state` holds the task text, the system and routing instructions, and the role. It holds no account names, quota, usage or credentials.
2. **Complexity is code's input, not the decider's choice.** Complexity = `score / 4 × 100`. The only gate is `router.policy.min_confidence`. `min_margin` is deprecated and ignored.
3. **Operators define levels.** `[[router.levels]]` maps ranges of complexity to a level, and each level lists its choices, optionally with a `priority`, or a model `tier` when it lists none. The defaults are low 0-37, mid 38-62 and high 63-100, all by tier.
4. **Code picks the choice.** The level comes from the complexity. Among the level's eligible choices, the lowest `priority` wins, then the most headroom on the choice's best account, then the fewest active runs, then config order. The account is chosen as before.
5. **A heuristic stands in** when there is no key, when the answer is rejected or the gate fails, when only one choice is eligible, or when the decider is unreachable and `on_unreachable = "heuristic"`. It maps a keyword and length estimate to 20, 55 or 85.

## Considered options

- **Keep the choice-picking decider and improve the descriptions.** Rejected. Descriptions and account context would still decide the answer, and every config change would move it.
- **Ask for a categorical level (four labels, as ComplexityRouter does).** Simpler to read, but it fixes the scale to the labels and cannot express the operator's own ranges or their choice lists. The score's probabilities also give a smooth number to map.
- **Keep `min_margin` as a second gate.** Rejected. A score has no margin between choices, so the gate measured nothing.

## Consequences

- **Privacy.** No account names, quota or usage leave the machine on a route. The decider sees the task text, the role and one instruction.
- **Operator-owned mapping.** Routing quality now depends on the levels and their choice order. `route eval` checks `expected_choices` against the pick, so its accuracy measures the levels as well as the decider. A missed case can mean a level is wrong, and the docs say to check the level before changing the decider.
- **Tuning is reset.** `min_confidence` (0.45) was tuned for the choice-picking decider. It needs re-tuning with `route eval --require-live` under the new question.
- **Descriptions.** Choice descriptions no longer reach the decider. They stay in `config.toml` for people and for the installed skill.
- **Old configs.** `min_margin` still parses and is ignored. Any `router.instruction` that asks the decider to choose a choice must be replaced with the new default. Old `route_decision` events still load, because the new result fields default to absent.
- **Naming.** "Pool" already means a metered slice of an account's usage (CONTEXT.md). The operator's word for a level's choice list was "pool", so the config key is `choices` and the docs say "the level's choices". A level's choices are not a pool. "Tier" already names the catalog's model class, so the level's `tier` key refers to that class and a range of complexity is always a "level".
