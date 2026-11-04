# Web UI and TUI feature map

The [web UI](web-ui.md) and the [terminal UI](tui.md) cover the same ground, and some features live in only one of them. This page lists each feature, where to find it and why a gap exists. A gap is either **intentional** (the surface cannot or should not do it) or a **follow-up** (not built yet). Anything missing from both is still available from the CLI or the MCP tools.

## Tab names and order

The web nav is Dashboard, Sessions, Accounts, Levels, Agents, Workflows, Settings. The session timeline opens from Sessions or the Dashboard and is not a nav item. The TUI tabs are Dashboard (`1`), Sessions (`2`), Runs (`3`), Accounts (`4`), Usage (`5`), Levels (`6`), Models (`7`), Settings (`8`) and Setup (`9`).

The names line up where the feature is the same. They differ for these reasons:

- **Timeline and Runs.** The web draws a Gantt timeline per session and opens a run in a side panel. A terminal cannot draw that, so the TUI lists runs and opens a run in a scrolling detail pane. Sessions also lists a session's runs in the TUI.
- **Usage and Models.** The web shows headroom, reset times and the cost chart inside the Dashboard and Accounts pages, and the model catalog inside the Levels page. The TUI gives each its own tab because one screen cannot hold them next to the tables they belong to.
- **Setup.** Only the TUI has it (see the table).
- **Agents and Workflows.** Only the web has pages for them.

## Feature map

| Feature | Web UI | TUI | Notes and gap reason |
| --- | --- | --- | --- |
| Dashboard stats | Dashboard: stat cards, accounts, live runs, escalations | Dashboard (`1`): counters, setup checklist, recent runs | Same data, different layout. The web has escalations and the cost chart; the TUI has the setup checklist. |
| Usage chart and headroom with reset times | Dashboard and Accounts: headroom bars with reset times (tooltip), cost and tokens per day chart | Usage (`5`): 5-hour and weekly headroom bars per account and pool with "resets in", 14-day usage sparkline | Same data. The web chart shows cost and tokens; the TUI sparkline shows daily usage only. |
| Cooldowns | Dashboard and Accounts: "cooling down until <time> (in 2h 10m)" badge | Usage (`5`): cooldowns with their end time; Accounts (`4`): `f` flags or clears one | Parity. |
| Sessions list | Sessions: filter by active, escalated, failed | Sessions (`2`): newest first, with run count and active runs | Gap, follow-up: the TUI has no filters. |
| Run timeline and detail (events, transcript) | Timeline: Gantt per session, run panel with brief, route decision, events, transcript, result, worktree | Sessions (`2`) and Runs (`3`): run detail with brief, route decision, events, transcript tail, result, diff | Gap, intentional: no Gantt chart in a terminal. |
| Send message | Run panel: message to a run | Not available | Gap, follow-up. The `send_message` MCP tool and the web UI cover it. |
| Fork | Not available | Not available | Gap, follow-up on both. Use the `fork_agent` MCP tool. |
| Accept, reject, cancel | Run panel: accept (without merge), reject with feedback, cancel | Runs (`3`): `a` accept and merge, `A` accept only, `r` reject, `c` cancel | The TUI can also merge; the web accepts without merging (SPEC 7.5). |
| Join a run | Run panel Join tab shows the command to copy | Runs (`3`): `o` steps aside and starts the harness | Gap, intentional: a browser cannot open a terminal. |
| Accounts: add, sign in, flag, import | Accounts: add, import, edit cap, priority and pools, enable, flag, remove, sign in (shows the command or URL and code) | Accounts (`4`): `a` add, `l` sign in (hands over the terminal), `e`, `f`, `x`, `i` import | Parity. Sign-in differs in form: the web shows a command or a device code, the TUI runs the login. |
| Levels edit | Levels: add, remove, reorder models, edit ranges | Levels (`6`): same edits | Parity. |
| Route preview | Levels, Preview tab | Not available | Gap, follow-up: left out of the TUI to avoid conflicts with the Levels insights work. Use `superorchestrator route` meanwhile. |
| Route log and records | Levels, Route log tab | Not available | Gap, follow-up. Use `runs list` or `route eval`. |
| Routing instruction | Levels, Routing instruction tab | Not available | Gap, follow-up. Edit `router.instruction` with `config set`. |
| Models catalog and refresh | Levels page: catalog per harness, Refresh | Models (`7`): catalog per harness, `r` refresh | Parity. |
| Decider settings and Test decider | Settings, Decider tab | Settings (`8`): edit fields, save and test | Both use the supervisor ([config](config.md#decider)); the TUI tests the built-in sample. |
| Other config settings | Settings: General, Notifications, Worktrees, UI, System | Settings (`8`): read-only summary (config path, UI bind, caps) | Gap, follow-up: edit with the web UI or `config set`. |
| Worktrees list and prune | Settings, System tab | Not available | Gap, follow-up. Use the `worktree` CLI command. |
| Agents | Agents: list, view, edit agent files | Not available | Gap, follow-up. Edit the files or use `agent` commands. |
| Workflows | Workflows: saved workflows, runs, step status | Not available | Gap, follow-up. Use the `workflow` commands or `run_workflow`. |
| Doctor | Settings, System tab | Setup (`9`): `doctor` step | Parity. |
| Setup wizard, init, install | Not available | Setup (`9`) | Gap, intentional: these write files and harness config on the local machine and run before the supervisor and the web UI exist. |
