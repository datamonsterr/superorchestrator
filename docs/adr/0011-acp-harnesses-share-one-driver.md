# ACP harnesses share one driver and a per-harness spec table

Status: accepted · 2026-11-03

## Context

Many subscription CLIs now speak the Agent Client Protocol (ACP, https://agentclientprotocol.com): JSON-RPC 2.0 over the child's stdio. The client sends `initialize`, `session/new` (or `session/load` / `session/resume`), `session/prompt` and the `session/cancel` notification. The agent streams `session/update` notifications and calls back into the client with `session/request_permission` (and `fs/*`, `terminal/*` when the client declares those capabilities).

We want GitHub Copilot CLI, Gemini CLI, Kimi Code, Cursor CLI and Qwen Code as harnesses. Writing one bespoke driver per CLI, as for codex, opencode, claude and agy, would repeat the same protocol code five times.

## Decision

1. **One `AcpDriver`** in `so-harness` (`src/acp/`) implements the protocol once. A static **ACP spec table** (`AcpSpec`, one row per harness) holds the launch argv, model flag, effort flag, full-permission flags, join flag and extra error patterns. Accepted efforts live on `Harness` in `so-core`; the credential-home variable lives in `so-accounts`. A new ACP harness is mostly a new row plus a `Harness` variant.
2. **Each CLI is a distinct `Harness` variant**: `copilot`, `gemini`, `kimi`, `cursor`, `qwen`. Model refs read `copilot/<model>[/effort]`, accounts, caps and routing work per harness, and nothing in the router or the store has to learn about ACP.
3. **We hand-roll the JSON-RPC client** with `serde_json`, as the codex driver does, instead of depending on the `agent-client-protocol` crate (5.0.0 at the time of writing). The crate has had five major versions, brings its own connection runtime built on `futures`, and pulls a schema crate and more into `cargo deny`. We need about ten messages, and field names follow the published schema.
4. **Protocol choices:**
   - The client declares no `fs` and no `terminal` capability, so the agent uses its own tools. Any agent-to-client request the driver does not implement is answered with JSON-RPC error -32601.
   - `session/request_permission` is answered at once with the first option whose `kind` is `allow_always`, else `allow_once` (SPEC 4: children run with full permissions). With no allow option the outcome is `cancelled`. Where the CLI has a flag that skips approvals, the spec table passes it too.
   - The superorchestrator MCP server goes into `session/new` `mcpServers` as a stdio server (`env` as `[{name, value}]`), so runs need no per-CLI MCP config file.
   - ACP has no system prompt. The run's system addendum (SPEC 7.4) and the agent profile's instructions are sent as a leading text block of the first prompt.
   - The model is passed with the CLI's model flag at launch where it has one. Otherwise the driver calls `session/set_config_option` on the option whose `category` is `model`; Kimi uses this because `kimi acp` takes no model flag.
   - The handshake waits up to 120 s per message. A -32000 error during the handshake is an auth failure. The native session id is the ACP `sessionId`. Resume uses `session/load` when the agent advertises `loadSession` (its replayed history is not re-emitted as new events), else `session/resume` when `sessionCapabilities.resume` is advertised, else the launch fails with a clear error.
   - A message sent while idle starts a new `session/prompt`. ACP has no steer, so a message sent mid-turn is queued and sent as the next prompt when the current one returns. Queued messages are still sent after an interrupt so no parent message is lost.
   - Interrupt sends `session/cancel` and waits up to 5 s for the prompt to return `stopReason: cancelled`.
   - Structured output: ACP has no schema option. The driver validates the final assistant message against the schema and re-prompts with the validation error up to 5 times, then fails the turn.
   - `stopReason: refusal` finishes the turn with `ErrorKind::Refused`. JSON-RPC errors and stderr are classified with `classify_common` plus the row's extra patterns; -32000 (auth required) is `Auth`.
5. **Headroom is unknown** for all five. None documents a quota API, so usage comes from error classification plus cooldown, and the UI shows headroom as unknown. A probe is added only where a real source exists.

## Consequences

- opencode and agy could later move to ACP (`opencode acp`, and agy if it gains `--acp`), which would delete transport code. That is a separate decision; this ADR does not migrate them.
- Facts about each CLI that ACP does not cover (credential env, signed-in files, flags) live in the spec table and in SPEC 4 and 5.2, and are checked against the installed CLI, not written from memory.
- Mid-turn messages arrive one turn later than on codex, which can steer.
