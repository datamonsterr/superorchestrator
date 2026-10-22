# The web UI

The web UI runs inside the supervisor. It shows what superorchestrator is doing and lets you change settings. It uses the same JSON API as the CLI, under `/api/v1`.

## Start it

```sh
superorchestrator ui
```

This starts the supervisor if needed, prints the URL (default `http://127.0.0.1:7878`) and opens your browser. Options: `--bind ADDR`, `--port N`, `--token T` and `--no-open`. If the UI already runs, the options are ignored.

## Pages

- **Dashboard.** Accounts with sign-in state, 5-hour and weekly headroom per pool, active runs against the cap, live runs, recent escalations, and cost and tokens per day.
- **Sessions.** Orchestrator and interactive sessions. Filter by active, escalated or failed.
- **Session timeline.** One lane per run, nested under its parent, updated live. Click a lane for the run panel: brief, route decision, events, transcript, result, worktree, join command, and actions (message, cancel, accept, reject).
- **Accounts.** Add, import, edit cap, priority and pools, enable or disable, flag a cooldown, remove, and sign in.
- **Choices.** Edit choices and the routing instruction, preview a route, read the decision log.
- **Agents.** List, view and edit agent files.
- **Workflows.** Saved workflows, runs and step status.
- **Settings.** `config.toml` fields, notifications, worktree defaults and the key source for Jev. The key value is never shown.

## Hosting and the token

The UI binds to loopback by default. Anything that can reach the port can drive your accounts, so treat a wider bind with care.

To bind to another address, set a token. A non-loopback bind without a token is refused.

```sh
superorchestrator ui --bind 0.0.0.0 --token "$(openssl rand -hex 24)"
```

The browser asks for the token once, then keeps a session cookie (`so_token`, HttpOnly, 7 days). Scripts can send `Authorization: Bearer <token>`. Failed logins are rate limited.

To keep the token, set `ui.token` in `config.toml`. It is stored in plain text in that file (mode 0600). The API cannot change it. Even on loopback, a set token is enforced. For access from other machines, prefer an SSH tunnel or a reverse proxy with TLS. If a trusted proxy terminates TLS, the cookie is marked `Secure` only when that proxy sends `X-Forwarded-Proto: https`.

The UI checks the `Host` and `Origin` headers against DNS rebinding and cross-site requests. Credential homes and auth files are never served.

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
