# A per-user supervisor process owns all runs

Each orchestrator conversation starts its own `superorchestrator mcp` over stdio, so runs hosted inside that process would die when the conversation closes, could not be joined live, and would be invisible to the web UI. We run one per-user supervisor (`superorchestrator daemon`), started on demand by the first `mcp` or `ui` call and exiting after an idle period. It owns every harness process and serves the web UI. `mcp` processes are thin clients over a unix socket. This reverses the original "no daemon" goal on purpose.

## Considered Options

- Runs inside each `mcp` process: no daemon, but runs die with the conversation, and live join and the UI need a cross-process channel anyway.
- A system service installed at setup: always on, but heavier to install and to keep in step with the binary version.
