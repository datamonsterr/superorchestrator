# A per-user supervisor process owns all runs

Each orchestrator conversation starts its own `superorchestrator mcp` over stdio, so runs hosted inside that process would die when the conversation closes, could not be joined live, and would be invisible to the web UI. We run one per-user supervisor (`superorchestrator daemon`), started on demand by the first `mcp` or `ui` call and exiting after an idle period. It owns every harness process and serves the web UI. `mcp` processes are thin clients over a unix socket. This reverses the original "no daemon" goal on purpose.

## Considered Options

- Runs inside each `mcp` process: no daemon, but runs die with the conversation, and live join and the UI need a cross-process channel anyway.
- A system service installed at setup: always on, but heavier to install and to keep in step with the binary version.

## RPC identity and ownership (GHSA-vw56-37xq-jrf9)

An MCP connection opens an orchestrator session, or authenticates as an existing run with its run id and a per-run random token. The supervisor stores only a SHA-256 hash and compares it in constant time at `hello`. Spawned MCP configurations receive `SUPERORCHESTRATOR_RUN_ID` and `SUPERORCHESTRATOR_RUN_TOKEN`; a private `rpc-token` file in the run directory retains the token across resumes and supervisor restarts. Legacy runs without a token cannot claim run identity.

Session-bearing RPC calls cannot address runs or workflows from another session. A run caller's reads and controls are restricted to its own subtree, and messages can also go to ancestors for parent communication. The sessionless `inspect` connection used by the CLI/TUI remains a local operator connection with its existing read/cancel/join access. The 0600 socket is a per-UID boundary: this protects MCP session isolation, not mutually untrusted same-UID shell processes. See [SPEC section 20](../../SPEC.md#20-security) for the full method policy.
