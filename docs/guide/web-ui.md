# The web UI

The web UI runs inside the supervisor. It shows what superorchestrator is doing and lets you change settings. It uses the same JSON API as the CLI, under `/api/v1`.

## Start it

```sh
superorchestrator ui
```

This starts the supervisor if needed, prints a sign-in URL (default `http://127.0.0.1:7878/token#token=…`) and opens your browser. The fragment carries a random, single-use login code, never the long-lived token. The supervisor mints a fresh code for every `ui` call over its private socket. Codes expire after 60 seconds and are exchanged through the existing token-to-cookie endpoint. The browser removes the fragment from its current history entry before posting the fragment value unchanged. Options: `--bind ADDR`, `--port N`, `--token T` and `--no-open`. If the UI already runs, the options are ignored and a fresh login code is returned for that same UI.

The browser's global history may still record the original fragment. A history entry or copied URL cannot authenticate again after its code has been redeemed; copying the URL is safe after first use. An unused code expires after 60 seconds. Treat an unused, unexpired URL as a sign-in credential. Run `superorchestrator ui` again for a fresh code if needed.

`--no-open` prints the URL without starting `xdg-open` or `open`, avoiding URL exposure in opener argv entirely. When an opener runs, its argv contains only the short-lived code, never the persistent token. On shared machines, keep explicit tokens in the private config file.

## Pages

- **Dashboard.** Accounts with sign-in state, 5-hour and weekly headroom per pool, active runs against the cap, live runs, recent escalations, and cost and tokens per day.
- **Sessions.** Orchestrator and interactive sessions. Filter by active, escalated or failed.
- **Session timeline.** One lane per run, nested under its parent, updated live. Click a lane for the run panel: brief, route decision, events, transcript, result, worktree, join command, and actions (message, cancel, accept, reject).
- **Accounts.** Add, import, edit cap, priority and pools, enable or disable, flag a cooldown, remove, and sign in.
- **Choices.** Edit choices and the routing instruction, preview a route, read the decision log.
- **Levels.** A tab on the same page. Each complexity level shows its range, its tier and the models it allows (priority, choice, harness, model, effort, on/off/unknown). Add, edit and delete levels; add an existing choice to a level or create a new one for it; move a model up or down (this rewrites the priorities 1..n) or remove it; reset to the default levels. Every edit saves the whole list in one request and shows the server message if it is rejected. Without saved levels the tab says "Using the default levels" and lists the tier defaults. A banner appears when the saved levels differ from what the running supervisor loaded.
- **Agents.** List, view and edit agent files.
- **Workflows.** Saved workflows, runs and step status.
- **Settings.** `config.toml` fields, notifications, worktree defaults and the key source for Jev. The key value is never shown.

## Hosting and the token

The UI binds to loopback by default. Every bind requires a token, including loopback: other users on the same machine can reach loopback TCP. Requests without a valid session cookie or bearer token receive 401.

If neither `--token` nor `ui.token` is set (or the configured value is empty), startup generates a 256-bit token from the OS CSPRNG and saves it as `ui-token` in the config directory, with mode 0600. Restarts reuse that token, and subsequent `superorchestrator ui` calls return fresh single-use sign-in URLs for the running supervisor. Missing files are created atomically; malformed files, symlinks, files owned by another user, and files with broader permissions stop startup. The `ui_start` error (and `superorchestrator ui`) names the file, the reason (not 0600, a symlink, malformed or owned by another user) and the hint `remove <path> to regenerate it`. To recover, remove that file; the next start generates a new token and invalidates old cookies.

Bookmarks of the bare `http://127.0.0.1:7878` from earlier versions, which opened without a token, now show the token prompt. Open the UI with `superorchestrator ui` instead, which prints and opens a fresh sign-in URL.

To bind to another address, you may supply your own token:

```sh
superorchestrator ui --bind 0.0.0.0 --token "$(openssl rand -hex 24)"
```

Opening the CLI URL exchanges its single-use code through `POST /api/v1/token` (the UI uses its `/api/v1/auth` alias), then keeps a session cookie (`so_token`, HttpOnly, SameSite=Strict, 7 days). Opening the base URL manually shows the token prompt when no valid cookie is present. Scripts can read the generated token from the private file and send `Authorization: Bearer <token>`. Wrong tokens, used or expired login codes, bearer credentials and cookies count toward the per-IP limit of five failures per minute; a sixth attempt receives 429. Invalid cookies are also cleared. Valid API cookies and bearer credentials remain accepted while guesses are limited.

`--token` takes precedence over `ui.token`, and either overrides the generated file. To persist an explicit token, set `ui.token` in `config.toml` (mode 0600). The API cannot read or change the token, including the generated file. Changing the effective token invalidates existing cookies. For access from other machines, prefer an SSH tunnel or a reverse proxy with TLS. If a trusted proxy terminates TLS, the cookie is marked `Secure` only when that proxy sends `X-Forwarded-Proto: https`.

The supervisor keeps at most 64 pending login codes, removes expired codes, and evicts the oldest pending code when full. Redemption compares code values in constant time and consumes a successful match atomically. Login codes do not work as API bearer credentials.

The UI checks the `Host` and `Origin` headers against DNS rebinding and cross-site requests. Its CSP uses `style-src 'self'` and permits inline style attributes for dynamic bars, timeline positions and library layout. Inline style elements require a fresh document nonce (charts and Radix) or Sonner's exact stylesheet and empty-element hashes. Credential homes and auth files are never served.

## Run it as a service

```sh
superorchestrator ui --install-service
```

On Linux this writes a systemd user unit. On macOS it writes a launchd agent. The service runs `superorchestrator daemon --foreground` and keeps the supervisor up. The command also sets `ui.enabled = true`, so the supervisor serves the UI when it starts. It prints the command that enables the service. On Linux:

```sh
systemctl --user daemon-reload && systemctl --user enable --now superorchestrator.service
```

`--bind`, `--port` and `--token` with `--install-service` are saved to `config.toml`.

Without the service, the supervisor stops after `supervisor.idle_exit` (default 10 minutes) with no active runs, no MCP clients and no open UI streams.
