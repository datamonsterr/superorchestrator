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
