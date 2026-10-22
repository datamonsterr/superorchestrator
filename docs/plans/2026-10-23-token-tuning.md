# Token tuning of the MCP surface and skills (T10)

Goal: cut the tokens an orchestrator spends on `tools/list`, on tool responses and on the
skills, without losing information it needs.

## Measuring

`cargo test -p so-supervisor --test token_report -- --nocapture` runs a canonical flow against
a fake-harness supervisor (list_harnesses, route, spawn_agent with wait and review, get_run,
get_run with diff, spawn_agent without wait, wait_agent, accept_run with merge, run_workflow,
workflow_status) and prints the bytes of `tools/list` per tool and of each response. The "rpc"
column is the supervisor reply (what the MCP bridge forwarded before this work); "mcp" is what
the orchestrator receives. `SHOW=1` also prints every reply.

## Before (commit f763e0e)

| call | bytes |
| --- | ---: |
| tools/list (14 tools) | 9712 |
| list_harnesses | 1968 |
| route | 630 |
| spawn_agent (wait, review) | 90 |
| get_run | 4095 |
| get_run (diff) | 4420 |
| spawn_agent (no wait) | 82 |
| wait_agent | 84 |
| accept_run (merge) | 57 |
| run_workflow | 133 |
| workflow_status | 438 |
| responses total | 11997 |

tools/list per tool: run_workflow 2130, spawn_agent 1591, accept_run 1144, route 889,
get_run 721, send_message 601, wait_agent 478, workflow_status 406, reject_run 374,
fork_agent 340, cancel_agent 282, list_harnesses 270, read_messages 245, list_agents 241.

Skills: use-superorchestrator-mcp 3349 B (static plugin copy), superorchestrator-cli 4394 B.

## After

| call | before | after | change |
| --- | ---: | ---: | ---: |
| tools/list (14 tools) | 9712 | 5722 | -41% |
| list_harnesses | 1968 | 1345 | -32% |
| route | 630 | 249 | -60% |
| spawn_agent (wait, review) | 90 | 90 | 0% |
| get_run | 4095 | 303 | -93% |
| get_run (diff) | 4420 | 549 | -88% |
| spawn_agent (no wait) | 82 | 68 | -17% |
| wait_agent | 84 | 100 | +19% (now carries each finished run's state and result) |
| accept_run (merge) | 57 | 47 | -18% |
| run_workflow | 133 | 81 | -39% |
| workflow_status | 438 | 275 | -37% |
| responses total | 11997 | 3107 | -74% |

tools/list per tool (before to after): run_workflow 2130 to 1018, spawn_agent 1591 to 1090,
accept_run 1144 to 529, route 889 to 607, get_run 721 to 419, send_message 601 to 337,
wait_agent 478 to 400, workflow_status 406 to 242, reject_run 374 to 232, fork_agent 340 to 217,
cancel_agent 282 to 159, list_harnesses 270 to 219, read_messages 245 to 109, list_agents 241
to 142.

Other numbers: the server `instructions` string went from about 700 to 240 bytes. Skills:
use-superorchestrator-mcp (static plugin copy) 3349 to 1772 bytes (budget 1800),
superorchestrator-cli 4394 to 2482 bytes (budget 2500). The generated MCP skill adds one line
per account, choice and agent.

The `get_run` rows understate the gain on real runs: the fixture run has 11 events, a real one
has up to 50 (tool calls, usage), and the default view shows at most 5 key events.
`list_harnesses` is dominated by the free model list (12 ids, about 430 bytes); it is the part an
orchestrator needs to pick a free model without another call.

## What changed

- `so-mcp/src/tools.rs`: short descriptions; `schema_of` trims the generated schemas and
  `PARAM_DOCS` holds the few parameter descriptions that remain; `readOnlyHint` is sent only
  when true (the MCP default is false).
- `so-mcp/src/compact.rs`: reply projections (the supervisor RPC shapes are unchanged, so the
  CLI and the web UI are not affected). The bridge reads each finished run for `wait_agent`.
- `so-supervisor/src/handlers/harnesses.rs`: `list_harnesses` with `{"brief": true}` builds the
  short view next to the catalog and the load snapshot (eligibility uses the router's own
  `candidate`). The catalog is cached by the file time of `models.toml`.
- `so-router/src/catalog.rs`: `Catalog::is_free` and `free_models`; `:free` ids count as free.
- `so-agents/src/skill.rs` and `plugin/skills/`: terse skills, workflow first.

## Decisions

- Free models are listed once per harness (`free_models.opencode`), not repeated under each
  opencode account: the catalog is per harness, and three accounts would triple the list.
- `eligible` is left out when true and `why` appears only for ineligible choices.
- `get_run` takes `verbose` in `GetRunParams` (the supervisor ignores it) so that the schema
  documents it; `verbose` returns the supervisor's reply untouched.

## Review fixes

Results of finished runs, workflow steps and message bodies are never pruned; `get_run` keeps
the latest check result, failed-check gate, failover, escalation and stall beyond its window and
caps progress at 2; `wait_agent` entries carry `failed_checks`, `failover` and `question`, and an
unreadable run is `state: unknown` with the error. Schemas keep multi-arm unions. One free rule
(`so_core::model_name_is_free`, explicit catalog cost wins). `token_report` calls the bridge's
own `rpc_args` and `mcp_reply`. Numbers stay in budget: tools/list 5722, responses 3107.
