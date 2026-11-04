# Endpoint accounts

An endpoint account is a `claude` account whose Claude Code process talks to an Anthropic-compatible coding-plan API instead of an Anthropic subscription. Three coding plans are supported by preset: the Z.ai GLM Coding Plan, the Kimi Code plan and the MiniMax M Plan. A `custom` preset covers any other compatible URL. The design is in [ADR 0010](../adr/0010-endpoint-accounts.md).

Why use one: these plans are cheap capacity for subagents. The harness is still Claude Code, so launch, `runs join`, session handling and error handling work as for any `claude` account. Endpoint accounts are not orchestrator accounts, and routing spends them like any other harness.

## Add an account

The account name is the positional argument. The harness is always `claude`.

```
# paste the key on standard input; it is saved to a key file
superorchestrator account add glm-1 --endpoint zai --token-stdin

# or read the key from an environment variable
superorchestrator account add kimi-1 --endpoint kimi --token-env KIMI_API_KEY
```

Flags:

| Flag | Meaning |
|---|---|
| `--endpoint zai\|kimi\|minimax\|custom` | The preset. |
| `--token-stdin` | Read one line from standard input. It is saved to `<config dir>/keys/<name>.key`. |
| `--token-env VAR` | Name of a variable that holds the key. Only the name is saved. |
| `--base-url URL` | Override the preset's base URL. Required for `custom`. |
| `--model ID` | A model the account serves. Repeat for several. Required for `custom`. |
| `--opus ID`, `--sonnet ID`, `--haiku ID` | Override a model slot. All three are required for `custom`. |

The command form is `account add <name> --endpoint <preset> [--base-url URL] [--token-env VAR | --token-stdin] [--model ID ...] [--opus ID --sonnet ID --haiku ID]`. `--model` may be repeated. The equivalent TUI flow is `E` in Accounts: enter a name, choose a preset and token source, then provide the masked token or environment variable name and any custom endpoint fields.

Give exactly one of `--token-stdin` and `--token-env`. `--name`, `--count` and `--home` do not apply to endpoint accounts.

The web UI add-account dialog and the terminal UI add flow offer the same presets.

The resulting file has an `[endpoint]` table:

```toml
name = "glm-1"
harness = "claude"
home = "~/.glm-1"

[endpoint]
preset = "zai"
token_file = "~/.config/superorchestrator/keys/glm-1.key"
```

The full field list and validation rules are in [SPEC 5.1](../../SPEC.md). `base_url` must be `https://`, or `http://127.0.0.1` or `http://localhost` for a local mock. With `token_env`, the variable must be set and non-empty in the environment of the process that launches the harness: the supervisor for supervised runs, your shell for the wrapper. Restart the supervisor after you change its environment.

## What the harness sees

For an endpoint account the launch sets:

- `ANTHROPIC_BASE_URL` to the base URL.
- The key under the preset's key variable (`ANTHROPIC_AUTH_TOKEN` or `ANTHROPIC_API_KEY`). The other one is removed, so your own Anthropic key never reaches the provider.
- `ANTHROPIC_DEFAULT_OPUS_MODEL`, `ANTHROPIC_DEFAULT_SONNET_MODEL` and `ANTHROPIC_DEFAULT_HAIKU_MODEL` from the slots, and `ANTHROPIC_MODEL` set to the opus slot model.
- The preset's extra variables (Z.ai only).
- `CLAUDE_CONFIG_DIR`, so settings and sessions stay isolated per account.

Supervised runs, the wrapper (`superorchestrator exec`) and `runs join` build the launch the same way, so a joined session has the same environment as the run.

## Providers

Facts below were checked on 2026-11-03. None of the three providers documents a usage or quota API, so superorchestrator does not probe them. See [Quota and headroom](#quota-and-headroom).

### Z.ai GLM Coding Plan

- Preset: `zai`
- Base URL: `https://api.z.ai/api/anthropic`
- Key variable: `ANTHROPIC_AUTH_TOKEN`
- Models: `glm-5.3` (tier high), `glm-5.3-flash` (tier fast)
- Slots: opus and sonnet `glm-5.3`, haiku `glm-5.3-flash`
- Extra variables: `API_TIMEOUT_MS=3000000`, `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1`
- Docs: [Claude Code setup](https://docs.z.ai/devpack/tool/claude), [plan overview](https://docs.z.ai/devpack/overview), [error codes](https://docs.z.ai/api-reference/api-code)

Quota: the plan has a 5-hour window and a weekly or monthly window. Z.ai documents the errors for them (code 1308 for the 5-hour window, 1310 for the weekly or monthly window) but no usage API. For current allowances see the plan overview. The error message carries the reset time, for example `Your limit will reset at 2026-11-03 18:00:00`. The time is UTC+8, and superorchestrator uses it as the end of the cooldown.

Plan allowances are Lite 2,000 / 10,000, Pro 12,000 / 60,000, and Max 28,000 / 140,000 credits per 5 hours / per week. Code 1113 means “Insufficient balance”; the classifier does not map this code, so it falls back to the HTTP status unless another recognized phrase is present.

### Kimi Code

- Preset: `kimi`
- Base URL: `https://api.kimi.ai/coding/`. In China use `https://api.kimi.com/coding/` with `--base-url`.
- Key variable: `ANTHROPIC_API_KEY`
- Models: `k3.1-pro` (high), `k3.1-pro-highspeed` (mid), `k3` (mid), `kimi-for-coding` (mid)
- Slots: `k3.1-pro` for opus, sonnet and haiku
- Docs: [Claude Code setup](https://www.kimi.com/code/docs/en/third-party-tools/claude-code.html)

Quota: no numbers are documented. Kimi answers 429 with a message that names the window: "You've reached your 5-hour usage limit", "weekly (7-day) usage limit" or "monthly usage limit". A capacity problem answers "The engine is currently overloaded".

### MiniMax M Plan

- Preset: `minimax`
- Base URL: `https://api.minimax.io/anthropic`
- Key variable: `ANTHROPIC_AUTH_TOKEN`
- Models: `MiniMax-M3.1-Flash` (mid)
- Slots: `MiniMax-M3.1-Flash` for opus, sonnet and haiku
- Docs: [Claude Code setup](https://platform.minimax.io/docs/m-plan/claude-code.md)

Quota: no numbers are documented, and the MiniMax docs do not list the quota error codes. Third-party references name code 2056 for a usage limit and 1002 for a rate limit. Superorchestrator classifies those two, but they are unconfirmed.

### Custom

Use `custom` for any other Anthropic-compatible URL. Give everything a preset would supply:

```
superorchestrator account add proxy-1 --endpoint custom \
  --base-url https://llm.example.com/anthropic \
  --token-env PROXY_API_KEY \
  --model my-model \
  --opus my-model --sonnet my-model --haiku my-model
```

The key variable defaults to `ANTHROPIC_AUTH_TOKEN`. To use `ANTHROPIC_API_KEY`, set `token_var = "ANTHROPIC_API_KEY"` in the `[endpoint]` table of the account file. `--model` lists the models the account serves. A custom model is not in the catalog, so list its ref on a level yourself.

## Quota and headroom

There is no probe, so `account list` shows headroom from the local ledger only. When a run hits a quota error, the account's pool is marked exhausted until the reset time when the message gives one (Z.ai does), and for `usage.default_cooldown` (5 h by default) otherwise. The run then fails over to the next account or model; see [routing](routing.md#failover).

## Error codes

Provider errors are recognized from the response text. A code matches only as its own number, as in `"code":"1308"`, not inside a longer number.

| Class | Z.ai | Kimi | MiniMax |
|---|---|---|---|
| Quota (pool marked exhausted) | 1308, 1309, 1310, 1316–1321, "limit exhausted", "Usage limit reached for" | "5-hour usage limit", "weekly (7-day) usage limit", "monthly usage limit" | 2056 |
| Rate limit | 1302, 1305, 1312 | "The engine is currently overloaded", "too many requests", "concurrent request limit" | 1002 |
| Auth | 1000, 1001, 1003 | "membership benefits" | 2049 |

These are the provider codes the classifier maps. Z.ai 1113 (“Insufficient balance”) is unclassified by code. Other responses fall back to the HTTP status: 401 and 403 are auth errors, and 429 is a rate limit.

## Real-provider smoke

The ignored integration test `real_endpoint_smoke` reads exactly these environment variables: `SO_SMOKE_ENDPOINT_URL`, `SO_SMOKE_ENDPOINT_TOKEN_VAR`, `SO_SMOKE_ENDPOINT_TOKEN`, and `SO_SMOKE_ENDPOINT_MODEL`. If any is unset, it prints a skip message; otherwise it sends the validation request and requires it to succeed. The test is ignored by default.

## Make endpoint models routable

Routing picks only models that a level lists, or that a level's tier selects. Endpoint models are in the default catalog as `claude` models with no effort, so their refs have no effort segment:

| Preset | Refs |
|---|---|
| `zai` | `claude/glm-5.3`, `claude/glm-5.3-flash` |
| `kimi` | `claude/k3.1-pro`, `claude/k3.1-pro-highspeed`, `claude/k3`, `claude/kimi-for-coding` |
| `minimax` | `claude/MiniMax-M3.1-Flash` |

List them on a level (`superorchestrator config edit`):

```toml
[[router.levels]]
name = "simple"
min = 0
max = 40
models = ["claude/glm-5.3-flash", "claude/MiniMax-M3.1-Flash", "codex/gpt-6.1-sol/low"]
```

A level with no `models` and a `tier` selects every eligible model of that tier, so endpoint models enter such a level through their catalog tier.

An endpoint account serves only the models of its preset (or its `models` list), and a plain Claude account never receives an endpoint model. Endpoint choices are not demoted by the [Claude rank](routing.md#claude-rank). Where `router.priority` orders harnesses, they rank just ahead of `claude`.

To check a pick: `superorchestrator route "task" --model claude/glm-5.3 --explain`. Pinning an endpoint account with no model uses its opus slot model.

`[orchestrator] reserved_accounts` applies to endpoint accounts too. A reserved endpoint account is excluded from automatic routing and is eligible only when the call pins it.

## Check sign-in

An endpoint account is signed in when its key resolves: the variable is set and non-empty, or the key file exists with mode 0600 or 0400. CLI `account login`, TUI `l`, and web `POST /accounts/:name/login` all call the shared `so_harness::endpoint::validate_login`: one `POST {base_url}/v1/messages` with `max_tokens: 1` on the haiku slot model. The web POST validates and caches the result; `GET /accounts/:name/login` returns the cached result and does not validate again. The call uses one token of plan usage.

## Security

- The key is never stored in the account file, never printed and never put on a command line. Use `--token-stdin` or `--token-env`.
- Pasted keys go to `<config dir>/keys/<name>.key`. The directory is mode 0700 and the file is mode 0600. Reads accept mode 0600 or 0400; the target must be a regular file, and a symlink to a regular file is accepted.
- Log redaction covers `ANTHROPIC_AUTH_TOKEN` and `ANTHROPIC_API_KEY` values and the Z.ai key shape. The launch's debug output shows `[REDACTED]` for the key. Treat the log as sensitive anyway.
- The launch removes the other Anthropic key variable, so an operator key cannot leak to the provider.

See also [security](security.md).

## Troubleshooting

**401, or signed out in `account list`.** The key is wrong or expired, or it does not resolve. For `token_env`, check the variable in the process that launches the harness, and restart the supervisor if you changed its environment. For a key file, check that it is a regular file with mode 0600. Then run `superorchestrator account login <name>`.

**Wrong base URL or model.** Compare `base_url` with the provider sections above. Check that the slot models and `models` are ids the provider serves. A custom account has no defaults.

**1308 or 1310 from Z.ai.** The 5-hour or the weekly or monthly limit is used up. The account is on cooldown until the reset time in the message. Add another account, or wait.

**Kimi "reached your 5-hour usage limit".** The account is on cooldown for `usage.default_cooldown`, unless the message gives a reset time.

**An endpoint model is never picked.** No level lists or selects it, or no enabled, signed-in endpoint account of that preset exists. Run `superorchestrator route "task" --model claude/glm-5.3 --explain`.
