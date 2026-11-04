# Endpoint accounts run the claude harness against Anthropic-compatible APIs

Status: accepted · 2026-11-03

## Context

Several coding-plan subscriptions are served through Anthropic-compatible HTTP APIs that Claude Code can call when `ANTHROPIC_BASE_URL` and a key are set: the Z.ai GLM Coding Plan, the Kimi Code plan and the MiniMax M Plan (formerly Coding Plan). They are cheap subagent capacity. Today a `claude` account always means an Anthropic subscription, Claude choices rank last (SPEC 8.4), and the router has no notion of which models an account can serve beyond its harness and pool.

Provider facts, checked on 2026-11-03:

| Preset | Base URL | Key variable | Models | Source |
|---|---|---|---|---|
| `zai` | `https://api.z.ai/api/anthropic` | `ANTHROPIC_AUTH_TOKEN` | `glm-5.3`, `glm-5.3-flash` | docs.z.ai/devpack/tool/claude, /devpack/overview |
| `kimi` | `https://api.kimi.ai/coding/` (China: `https://api.kimi.com/coding/`) | `ANTHROPIC_API_KEY` | `k3.1-pro`, `k3.1-pro-highspeed`, `k3`, `kimi-for-coding` | kimi.com/code/docs/en/third-party-tools/claude-code.html |
| `minimax` | `https://api.minimax.io/anthropic` | `ANTHROPIC_AUTH_TOKEN` | `MiniMax-M3.1-Flash` | platform.minimax.io/docs/m-plan/claude-code.md |

None of the three documents a usage or quota API. Z.ai and Kimi document their quota errors: Z.ai returns HTTP 429 with a business code in `{"error":{"code":"1308","message":"Usage limit reached for 5 hour. Your limit will reset at ..."}}` (1308 five-hour, 1310 weekly or monthly, 1309 expired, 1316 to 1321 extra usage unavailable, 1302 and 1305 rate or overload; docs.z.ai/api-reference/api-code). Kimi returns 429 with "You've reached your 5-hour usage limit", "weekly (7-day) usage limit", "monthly usage limit", or "The engine is currently overloaded" for capacity. MiniMax quota errors are not in its docs; third-party references name code 2056 (usage limit) and 1002 (rate limit).

## Decision

1. **Endpoint on the account.** A `claude` account may carry an `[endpoint]` table: `preset` (`zai`, `kimi`, `minimax` or `custom`), `base_url` (preset default, required for `custom`), exactly one token source (`token_env`, the name of a supervisor environment variable, or `token_file`, a key file that must be mode 0600), optional `token_var` (preset default), optional `models` (required for `custom`), and optional `[endpoint.slots]` mapping `opus`, `sonnet` and `haiku` to model ids. The token value never appears in account config. Only `claude` accounts may have an endpoint. Such an account is an **endpoint account**.
2. **Launch.** `build_launch` adds `ANTHROPIC_BASE_URL`, the token under `token_var`, `ANTHROPIC_DEFAULT_OPUS_MODEL`, `ANTHROPIC_DEFAULT_SONNET_MODEL`, `ANTHROPIC_DEFAULT_HAIKU_MODEL` and the preset's extra variables (Z.ai: `API_TIMEOUT_MS=3000000`, `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1`), and removes the other key variable so an operator's own Anthropic key cannot leak into the request. The credential home and `CLAUDE_CONFIG_DIR` stay, so settings and sessions are isolated per account. Supervised runs, the wrapper and `join` all go through `build_launch`, so they get the same environment. The token is resolved at launch, not stored.
3. **Routing.** Catalog entries for the `claude` harness gain an optional `endpoint` preset. An account serves a `claude` model when: it has no endpoint and the model has no catalog endpoint (Anthropic models, and unknown models as before); or it has an endpoint and the model is in its `models` list, or, when `models` is empty, the model's catalog endpoint equals its preset. Plain Claude accounts never receive endpoint models, and endpoint accounts never receive Anthropic models. The Claude rank (SPEC 8.4) applies only to choices served by plain Claude accounts; endpoint choices compete like any other harness. Endpoint accounts are not orchestrator accounts.
4. **Headroom.** There is no documented usage API, so there is no probe. A community-known Z.ai monitor URL exists but is undocumented and is not used. Headroom for endpoint accounts comes from the ledger, and quota errors mark the pool exhausted with the default cooldown, or until the reset time when the message gives one.
5. **Sign-in.** An endpoint account is signed in when its token resolves (variable set and non-empty, or key file present with mode 0600). `account login` for an endpoint account sends one `POST {base_url}/v1/messages` with `max_tokens: 1` on the haiku slot model and reports the classified result.
6. **Key files.** The CLI, web UI and TUI write a pasted token to `<config dir>/keys/<account>.key`, directory 0700, file 0600, and set `token_file` to it. Redaction covers the token variables and the Z.ai key shape.

## Consequences

- Model refs such as `claude/glm-5.3` and `claude/k3.1-pro` become routable once a level lists them or a tier picks them.
- Endpoint model entries list no efforts, so their refs have no effort segment.
- The preset table is code. A new provider model needs a catalog entry or an explicit `models` list on the account.
- OpenCode is not used for these providers; one harness keeps launch, join and error handling in one place.
- No real end-to-end smoke ran when this was written: no provider key was available. A mock Anthropic-compatible server covers the launch path in an ignored integration test.
