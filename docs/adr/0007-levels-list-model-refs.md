# Levels list model refs; agent profiles replace roles

Status: accepted · 2026-10-25

## Context

Routing had two lists of the same thing. `[[choices]]` in `config.toml` defined each routable harness, model and effort combination, with `enabled`, `roles`, `description` and an id. Each `[[router.levels]]` entry then named those choices by id, with a `priority`. Adding a model meant editing the choices table and then the level, and the two could drift apart. The levels needed only the list of models in preference order.

Roles tied an agent profile to a model. A role selected which choices were eligible (`roles = ["worker"]`), so a profile such as the designer could only run on the models its role had been given. A task that needed a different model for the same kind of work had no way to say so except by changing the choices.

## Decision

1. **Levels list model refs.** Each `[[router.levels]]` entry has `models = ["harness/model/effort", ...]` in preference order. A model ref names the harness, model and effort in one string, and a model is enabled by being listed. `[[choices]]` and `choices = [...]` are no longer read. `tier` applies only when `models` is empty. A level needs `models` or `tier`.
2. **Agent profiles replace roles.** A profile is an agent file, named by `spawn_agent(agent: "<name>")`. It is orthogonal to the model: any profile can run on any model, by passing `model` on the call or setting `model:` in its file. The decider receives the profile name as `agent`. Roles are removed from config, `route`, `spawn_agent`, the CLI and the web UI.
3. **One account rule.** Among a harness's enabled accounts below their cap whose pool is available, the router takes the most headroom for the pool, then the fewest active runs, then the higher account `priority`, then the least recently used, then name. The rationale states the criterion that decided it.
4. **Model refs in pins.** `model` accepts a ref wherever it accepts a name. A ref pins harness, model and effort, and a conflicting `harness` or `effort` is an error.

## Consequences

- There is no compatibility for `[[choices]]`. A config that still has it fails to load, and the error message shows the new form and points to `docs/guide/routing.md`. The message is the migration guide.
- `choice_id` values are model ref strings everywhere: runs, `route_decision` events, route results and `list_harnesses`. Stored runs from before this change keep their old ids.
- The `route_decision` event key `role` becomes `agent`, and `route --role` is removed.
- `migrate` no longer has a choices step.
- Routing order is an operator decision. There are no priorities inside a level, so `route eval` accuracy depends on the order of each level's models.
- The `enabled` switch for Claude is gone. Claude is reachable only when a level lists a Claude ref, and it still ranks last unless a call names it (SPEC 8.4).
