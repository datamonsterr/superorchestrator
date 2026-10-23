# Security model

Superorchestrator runs coding agents with full permissions on your machine. This page says what it protects, and what it leaves to you.

## Credentials

- Credentials are never printed, logged, copied into prompts, served by the UI or committed.
- The sign-in check and the usage probes only test that an auth file exists. They never read its contents.
- Credential homes are linked, not copied. `account remove` keeps the home unless you pass `--purge`, and a purge refuses homes outside `$HOME`, harness default homes, and anything that holds the config dir.
- The supervisor log redacts text that matches known key patterns. Treat the log as sensitive anyway.
- The Jev key and the Discord webhook URL come from the environment or a dotenv file. They are not stored in `config.toml`, and the UI never shows them. The one secret in `config.toml` is `ui.token`, if you set it. The file is mode 0600.

## Local interfaces

- The supervisor socket has mode 0600 in a directory only you can use. Anyone who can write to it can start runs, so keep it private.
- The trust boundary is the operating-system UID: a same-UID process can still run `superorchestrator ui` or open a new inspect connection. Within the protocol, `ui_start` requires a sessionless, runless inspect/operator connection. MCP sessions and subagent/run connections are rejected before a login code can be minted. A run-scoped hello still carries its run and session context when it requests `inspect: true`, so that request is rejected too.
- The web UI binds to loopback by default and requires authentication on every bind, including loopback: other local users can reach loopback TCP. Without `--token` or `ui.token`, the supervisor generates a 256-bit OS-random token in `ui-token` under the config directory (owner-only, mode 0600) and reuses it on restart. `superorchestrator ui` gets a fresh, single-use 60-second login code over the private supervisor socket and opens a URL containing that code, never the persistent token. The browser removes the fragment and exchanges the code for the existing HttpOnly session cookie. Used or expired codes cannot authenticate; `--no-open` avoids exposing the URL through opener argv. Unused, unexpired URLs are sign-in credentials. See the [web UI guide](web-ui.md). The UI checks `Host` and `Origin`, rate limits invalid tokens, bearer credentials and cookies, and sends a content security policy. At capacity the limiter evicts the least recently failed IP instead of resetting all records.
- The UI CSP permits inline style attributes needed by React and layout libraries, while inline style elements require a fresh per-document nonce or the exact hashes of Sonner's fixed stylesheet and its initially empty style element. External styles come from the same origin; arbitrary inline stylesheets and scripts are blocked.
- Codex app-server sockets bind to `127.0.0.1` with a per-run token. `opencode serve` uses a per-run password.

## Subagents run with full permissions

A subagent can read and write anything your user account can, and run any command. The harness is told never to wait for a person, so it does not ask. Limit the damage like this:

- **Worktrees.** `isolation: worktree` is the default. A run works in its own git worktree and branch. `isolation: none` must be set on purpose, per call or per agent.
- **Accounts.** Subagents use the credentials of the account they run on. They can reach files that account's tools can reach.
- **Depth and caps.** Runs nest two deep. `limits.global_cap`, `limits.max_children` and each account's `cap` bound the load.
- **Custom agents.** `tools` and `disallowedTools` do not restrict every harness. See [custom agents](agents.md#per-harness-gaps).

## Task text

Task text goes to subagents as argv elements or JSON, never through a shell string. It cannot change which choices are eligible. The one place a shell command runs is the `checks` option, for example `cmd:make test`. The orchestrator writes it in the `spawn_agent` call. It runs with your privileges, outside the harness sandbox. It is never built from task text or run output. Treat `checks` like a command you would run yourself.

## Destructive commands

`account remove`, `worktrees prune` and `migrate --apply` ask for confirmation. Pass `--yes` to skip it. `install`, `uninstall` and `migrate` back up every file they change.

## Network

Superorchestrator contacts OpenRouter (Jev), models.dev (`models refresh`) and Discord (notifications), only when you set them up. The harnesses talk to their own services. There is no telemetry.

## Reporting a problem

Open an issue on the GitHub repository. For a security problem, do not post secrets or logs. Say what you found, and ask for a private way to share details.
