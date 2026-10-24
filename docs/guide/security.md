# Security model

Superorchestrator runs coding agents with full permissions on your machine. This page says what it protects, and what it leaves to you.

## Credentials

- Credentials are never printed, logged, copied into prompts, served by the UI or committed.
- The sign-in check and the usage probes only test that an auth file exists. They never read its contents.
- Antigravity launches and usage probes select their credential home with `--gemini_dir=<home>`. This puts the path in process arguments, where local process inspection (including `/proc/<pid>/cmdline` on Linux) may reveal it. This is a known trade-off of the harness interface: the argument contains a directory path, not credentials. Newly created credential homes use mode 0700; keep imported homes private too. Directory permissions protect their contents but do not hide the path in argv.
- Credential homes are linked, not copied. `account remove` keeps the home unless you pass `--purge`, and a purge refuses homes outside `$HOME`, harness default homes, and anything that holds the config dir.
- The supervisor log redacts text that matches known key patterns. Treat the log as sensitive anyway.
- The Jev key and the Discord webhook URL come from the environment or a dotenv file. They are not stored in `config.toml`, and the UI never shows them. The only secret `config.toml` can hold is `ui.token`, if you set it; the file is mode 0600. Otherwise the web UI token is generated into a separate mode 0600 `ui-token` file in the config directory, and each run's RPC token is a mode 0600 `rpc-token` file in that run's directory.

## Harness environment

Processes the supervisor launches (harness runs, usage and model probes, `doctor`
version checks) use an inherited-variable allowlist, followed by explicit account and
driver variables. Supervisor API keys and cloud credentials (`OPENAI_API_KEY`,
`ANTHROPIC_API_KEY`, `OPENROUTER_API_KEY`, `GH_TOKEN`, `SSH_AUTH_SOCK`, and so on) are
excluded unless named in `pass_env`. Interactive CLI commands keep your own
environment. The supervisor's other spawns (review `cmd:` and `gh` checks, git
worktrees, notifications, process inspection) are intentionally not isolated. See
[accounts](accounts.md#inherited-environment) for the allowlist, corporate CA
variables, loader-hook warning (`LD_PRELOAD`, `NODE_OPTIONS`) and opt-in
configuration. The baseline includes `XDG_*` and `DBUS_SESSION_BUS_ADDRESS`, so every
harness sees your config/data home variables and session bus. This limits accidental
environment disclosure; harnesses still run with the user's file access and
permissions.

OpenCode limitation: its [server authentication documentation](https://opencode.ai/docs/server/#authentication)
requires `OPENCODE_SERVER_PASSWORD` in the server environment and provides no
password-file or stdin option. The driver therefore sets a fresh password for each
run. OpenCode may pass this password to its own tool subprocesses; the supervisor
cannot clear another process's environment after startup. A supervisor-inherited
OpenCode password is excluded from other harnesses by default.

## Local interfaces

- The supervisor socket has mode 0600 in a directory only you can use. Anyone who can write to it can start runs, so keep it private.
- The trust boundary is the operating-system UID: a same-UID process can still run `superorchestrator ui` or open a new inspect connection. Within the protocol, `ui_start` requires a sessionless, runless inspect/operator connection. MCP sessions and subagent/run connections are rejected before a login code can be minted. A run-scoped hello still carries its run and session context when it requests `inspect: true`, so that request is rejected too.
- The web UI binds to loopback by default and requires authentication on every bind, including loopback: other local users can reach loopback TCP. Without `--token` or `ui.token`, the supervisor generates a 256-bit OS-random token in `ui-token` under the config directory (owner-only, mode 0600) and reuses it on restart. `superorchestrator ui` gets a fresh, single-use 60-second login code over the private supervisor socket and opens a URL containing that code, never the persistent token. The browser removes the fragment and exchanges the code for the existing HttpOnly session cookie. Used or expired codes cannot authenticate; `--no-open` avoids exposing the URL through opener argv. Unused, unexpired URLs are sign-in credentials. See the [web UI guide](web-ui.md). The UI checks `Host` and `Origin`, rate limits invalid tokens, bearer credentials and cookies, and sends a content security policy. At capacity the limiter evicts the least recently failed IP instead of resetting all records.
- The UI CSP permits inline style attributes needed by React and layout libraries, while inline style elements require a fresh per-document nonce or the exact hashes of Sonner's fixed stylesheet and its initially empty style element. External styles come from the same origin; arbitrary inline stylesheets and scripts are blocked.
- Codex app-server sockets bind to `127.0.0.1` with a per-run token. `opencode serve` uses a per-run password.

### Run identity and ownership

Every supervised run gets a random per-run RPC token. The supervisor stores only its hash, writes the token to `runs/<id>/rpc-token` (mode 0600) and hands it to the run's `superorchestrator mcp` process, which presents it in `hello`. The token is rotated when the run is resumed. A connection that says it belongs to a run without a matching token is rejected. After `hello`, the connection is bound to its session and run: it can see and act on its own session and the subtree of runs it spawned, and nothing else. A request for a run outside that scope is answered exactly like a request for a run that does not exist, so a caller cannot probe which ids exist. The operator (`inspect`) connection, used by the CLI, the terminal UI and the web UI, is not scoped.

Same-UID caveat: the boundary is the operating-system UID. A subagent runs as your user, so it can read a sibling run's `runs/<id>/rpc-token` from disk. Run isolation protects against protocol misuse and confused deputies, not against a hostile process with your file access.

## Subagents run with full permissions

A subagent can read and write anything your user account can, and run any command. The harness is told never to wait for a person, so it does not ask. Limit the damage like this:

- **Worktrees.** `isolation: worktree` is the default. A run works in its own git worktree and branch. `isolation: none` must be set on purpose, per call or per agent.
- **Accounts.** Subagents use the credentials of the account they run on. They can reach files that account's tools can reach.
- **Depth and caps.** Runs nest two deep. `limits.global_cap`, `limits.max_children` and each account's `cap` bound the load.
- **Custom agents.** `tools` and `disallowedTools` do not restrict every harness. See [custom agents](agents.md#per-harness-gaps).

## Task text

Task text goes to subagents as argv elements or JSON, never through a shell string. It cannot change which models are eligible. The one place a shell command runs is the `checks` option, for example `cmd:make test`. Only an orchestrator (depth 0, no calling run) may supply `cmd:` in a `spawn_agent` call. Subagent callers may supply only `ci` and `pr_threads`. It runs with your privileges, outside the harness sandbox. It is never built from task text or run output. Treat `checks` like a command you would run yourself.

Subagents cannot supply inline `script` to `run_workflow`; they can invoke saved workflows by `name` only from the user config directory. The supervisor never searches project or run directories for subagent workflow scripts, including files that would shadow a config workflow. A subagent's requested spawn `cwd` must canonicalise inside its calling run's worktree/cwd. All workflow steps must stay inside their workflow base, including after symlink resolution. These supervisor request restrictions do not sandbox the harness's own filesystem or shell access.

## Destructive commands

`account remove`, `worktrees prune` and `migrate --apply` ask for confirmation. Pass `--yes` to skip it. `install`, `uninstall` and `migrate` back up every file they change.

## Network

Superorchestrator contacts OpenRouter (Jev), models.dev (`models refresh`) and Discord (notifications), only when you set them up. The harnesses talk to their own services. There is no telemetry.

## Reporting a problem

Open an issue on the GitHub repository. For a security problem, do not post secrets or logs. Say what you found, and ask for a private way to share details.
