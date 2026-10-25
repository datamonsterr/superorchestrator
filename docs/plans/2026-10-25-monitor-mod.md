# Monitor mod: superorchestrator status inside Claude Code

Branch: `feat/monitor-mod`. Supersedes the unmerged `feat/timeline-mod` branch (left untouched).
Workflow: subagent-driven development through superorchestrator.

## Goal

A Claude Code mod, shipped in the repository's existing `plugin/` folder, that shows the superorchestrator state of the current conversation in a compact form:

- supervisor health, headroom (5-hour and weekly) per account and pool,
- the runs of this conversation's session with their state,
- one timeline chart: the supervisor, the orchestrator (this Claude conversation) and every subagent run as lanes, with durations, events, and which run needs Claude, needs the user, is stuck, failed or finished,
- drawn for the surface it runs on (terminal, desktop app, VS Code, mobile) and for the person's light or dark theme.

## Decisions

- The mod lives in `plugin/` beside the MCP server registration and the skills. One install gives both.
- Data comes from a new command, `superorchestrator monitor --json`, polled by the mod. The command reads the store and asks a running supervisor for its status. It never starts the supervisor.
- The timeline shows only this conversation's session. Headroom and supervisor state are global.
- Layout: a pane (sidebar) with the full monitor, a one-line band above the prompt only while something needs attention, a status line entry with counts, and toasts on attention transitions.
- Colors are Claude Code theme keys, so they follow the person's theme on every surface. Every state also has its own glyph, so color is never the only signal.

## Data contract: `superorchestrator monitor`

```
superorchestrator monitor [--json] [--session <ID>] [--client-pid <PID>] [--cwd <DIR>] [--window <MINUTES>]
```

`--window` defaults to 60 (minutes, 5 to 1440). The global `--json` flag works in either position, as for other commands. Without `--json` the command prints a short text summary: the supervisor line, the attention counts and one line per run (`<name> <state> <duration>`).

Session matching, first rule that matches wins:

1. `--session <ID>`: the session with that id or unique prefix. `matched_by: "id"`.
2. `--client-pid <PID>`: an active orchestrator session whose recorded `pid` (the `superorchestrator mcp` bridge) has a parent process equal to `PID` or equal to the parent of `PID`. This covers a mod running in the Claude Code process itself and one running in a child worker of it. Several matches: the newest `started_at`. `matched_by: "pid"`.
3. `--cwd <DIR>`: the newest active orchestrator session whose `cwd` equals `DIR`. `matched_by: "cwd"`.
4. No match: `session: null` and `runs: []`. Exit code 0.

`so_core::procinfo` gains `parent_pid(pid) -> Option<i64>`: Linux reads field 4 of `/proc/<pid>/stat` (after the closing parenthesis of the command name); other systems run `ps -o ppid= -p <pid>` under a fixed locale, as `process_start` does.

JSON output (one object; timestamps RFC 3339 UTC; percentages are integers 0 to 100 or `null`; a missing value is `null`, never left out):

```json
{
  "now": "2026-10-25T14:32:05Z",
  "window": { "from": "2026-10-25T13:32:05Z", "to": "2026-10-25T14:32:05Z" },
  "supervisor": {
    "running": true, "pid": 1644957, "uptime_s": 4000, "stale": false,
    "warning": null, "active_runs": 10, "sessions": 34
  },
  "accounts": [
    {
      "name": "codex-acc1", "harness": "codex", "enabled": true, "signed_in": "yes",
      "active_runs": 0, "cap": 2,
      "pools": [
        {
          "pool": "default", "status": "ok",
          "five_hour_pct": 64, "five_hour_resets_at": "2026-10-25T16:00:00Z",
          "weekly_pct": 51, "weekly_resets_at": "2026-10-29T00:00:00Z",
          "cooldown_until": null, "source": "probe", "observed_at": "2026-10-25T14:30:00Z"
        }
      ]
    }
  ],
  "session": { "id": "01a1...", "cwd": "/home/dat/dev/app", "started_at": "...", "matched_by": "pid" },
  "runs": [
    {
      "id": "01a16850-...", "name": "e3-core", "parent_run": null, "depth": 1,
      "harness": "codex", "account": "codex-acc3", "model": "codex/gpt-6.1-sol/high",
      "state": "pending_review", "attention": "claude", "question": null,
      "created_at": "...", "started_at": "...", "finished_at": null,
      "segments": [
        { "state": "queued", "from": "...", "to": "..." },
        { "state": "running", "from": "...", "to": "..." },
        { "state": "pending_review", "from": "...", "to": null }
      ],
      "marks": [ { "ts": "...", "kind": "check_failed" } ]
    }
  ]
}
```

Field rules:

- `supervisor.running: false` when no supervisor answers; its other fields are then `null`.
- `accounts`: every configured account, in config order. `pools` come from the headroom table; `cooldown_until` is the latest active flag for that account (the same source `status` prints as `cooldown until`). An account with no headroom row has one pool `{ "pool": "default", "status": "unknown", ... }` with null numbers.
- `runs`: the matched session's runs that are active, plus finished runs whose `finished_at` is inside the window. Tree order: a parent before its children, siblings by `created_at`. At most 50 runs; active runs are kept first when cutting.
- `model`: the run's model ref (`choice_id`) when set, else its `model`.
- `segments`: built from the run's `run_queued`, `run_started`, `state_changed{from,to}` and `run_finished` events. Each segment is one state over `[from, to)`. The last segment of an active run has `to: null`. A finished run's last segment ends at `finished_at`; its final state is the run's `state`. Segments that end before `window.from` are dropped, and a segment that starts before it is clipped to it.
- `marks`: events inside the window, mapped: `question_asked` to `question`, `nudge_sent` to `nudge`, `escalated` to `escalated`, `failover` to `failover`, `check_result` with `ok: false` to `check_failed`, `review_rejected` to `review_rejected`, `review_accepted` to `review_accepted`.
- `question`: the text of the latest `question_asked` with no `question_answered` after it, else `null`.
- `attention` (a pure function of state, open question and parent):
  - `pending_review` → `"claude"` (the orchestrator must accept or reject),
  - `waiting` with an open question and `parent_run: null` → `"claude"` (the question goes to the orchestrator),
  - `stalled` or `escalated` → `"stuck"`,
  - `failed` → `"failed"`,
  - anything else → `null`.
  `"user"` never comes from the command: only the mod knows when Claude Code waits for the person.

## States, tones and colors

Every lane segment and run mark maps to one tone. Theme keys are Claude Code's own (`ThemeKey` in the mod API), so terminal, desktop and VS Code follow the person's theme with no extra work.

| Tone | Comes from | Theme key | Terminal bar | End mark |
|---|---|---|---|---|
| `running` | run `starting`, `running` | `ide` | `█` | `▶` |
| `claude` | orchestrator working; run `pending_review`, question to the orchestrator | `claude` | `█` working, `▓` waiting for Claude | `◆` |
| `user` | orchestrator waiting on a permission prompt or `AskUserQuestion` | `bashBorder` | `█` | `●` |
| `stuck` | run `stalled`, `escalated` | `warning` | `▒` | `⚠` |
| `failed` | run `failed` | `error` | none | `✗` |
| `done` | run `succeeded` | `success` | none | `✓` |
| `waiting` | run `waiting` without a question for the orchestrator; orchestrator blocked in `wait_agent` | `inactive` | `░` | none |
| `queued` | run `queued` | `inactive` (dim) | `·` | `○` |
| `idle` | orchestrator between turns | `subtle` | `░` | none |
| `cancelled` | run `cancelled` | `inactive` | none | `■` |

Supervisor lane: `up` (`success`, `█`), `stale` build (`warning`, `▒`), `down` (`error`, `·` and the word `down`).

Headroom bars: 50 % or more `success`, 15 % to 49 % `warning`, under 15 % `error`; `cooldown until HH:MM` and `exhausted` in `error`; `unknown` as `—` in `inactive`. Ledger-only accounts (claude, opencode) show `ledger`.

## Lanes the mod records itself

- Orchestrator lane, from Claude Code events in this session:
  - `turn.start` opens a `claude` segment; `turn.complete` closes it and opens `idle`.
  - A `tool.call` for `mcp__superorchestrator__wait_agent` (or `spawn_agent` with `wait: true`) is `waiting` while it runs.
  - A `tool.call` for `AskUserQuestion`, and a `tool.check` that ends in a question to the person, is `user` until it settles. The implementer confirms in the API types how a person prompt shows at `tool.check` before relying on it.
- Supervisor lane: one sample per poll (`up`, `stale`, `down`); equal neighbours merge.
- Both lanes keep at most the window plus one hour of segments.

## Theme and surface detection

- Theme: read the `theme` row of `$.config.list()` at `session.start`, and again whenever a `config.set` for `theme` passes (observe with `await next(e)`, never deny). Derive `{ mode: 'dark' | 'light', ansi, daltonized }` from the value (`dark`, `light`, `dark-daltonized`, `light-daltonized`, `dark-ansi`, `light-ansi`; anything else counts as dark). Use: the empty track and empty headroom cells draw in `subtle` on dark themes and in dim `inactive` on light themes. Daltonized themes keep the glyph marks (always on) and theme keys remap the hues.
- Surface: each `ui.render` reads `e.surface`.
  - `terminal`: glyph timeline sized to `e.props.bodyColumns`.
  - `desktop`, `vscode`: Box bars whose widths are percentages of the window, `backgroundColor` set to the tone's theme key, labels in `mono` caption text.
  - `mobile`: counts and one line per run with its mark and duration; no bars.
- Placement: auto-open the pane at `session.start` only where it docks as a sidebar: `e.viewport?.isFullscreen === true` at a render, or `$.session.surfaces()` includes a non-terminal surface. On the terminal's main screen the pane opens inline only through `/so-monitor`.

## Layout

Terminal pane (about 40 columns):

```
Superorchestrator       ● up 1h06m
quota          5h        week
codex-acc1  ■■■■■■■■■ 64% ■■■■ 51%
codex-acc2  cooldown until 13:52
agy-acc1/g  ■■■■■■■■■ 12% low
+6 ok ≥50%
▶2 run  ◆1 Claude  ●1 you  ⚠1  ✗1
           -30m      -15m     now
supervisor ██████████████████████
claude     ████░░████████████░░▶
├ e3-core  ··█████████▓▓◆ 4m
├ e4-sales ····██████▒▒▒⚠ 6m
│ └ fix-ui ·······████▶ 2m
├ e4-rpt   ·█████████✓     9m
└ e4-e2e   ······███✗       1m
```

- Header: title, supervisor dot and uptime, or `supervisor down`, or a stale-build warning.
- Quota: worst pools first (exhausted, cooldown, low, then by lowest percent). Pools at 50 % or more on both windows fold into one `+N ok ≥50%` line when room is short.
- Counts line: only non-zero counts.
- Timeline: axis labels at the left edge, the middle and `now`. Label column `clamp(8, floor(columns * 0.3), 12)` wide, with tree prefixes `├ └ │` for depth. Duration column 6 wide: elapsed time for an active run, total time for a finished one (`45s`, `4m`, `1h06m`).
- Each run row is a plain `Button` keyed by run id. Pressing it toggles one detail line under it: model ref, account, branch, and the open question when there is one.
- A `Button` with hotkey `w` cycles the window: 30, 60, 180 minutes.
- Height: when `e.viewport.rows` is short, quota folds to one line (`quota: lowest agy-acc1 12% · 2 cooldown`) first, then runs are cut with `+N more`, keeping runs that need attention.

Desktop and VS Code pane: the same sections. Quota rows are a label, two bars and two percentages. Timeline rows are a label, a row of Box segments and the duration with its mark. A keyed row shows its detail line on hover.

Band above the prompt (only while some run has `attention`, the orchestrator lane is `user`, or the supervisor is down while the session has active runs):

```
SO  ◆ 1 review  ● 1 you  ⚠ 1 stuck  ✗ 1 failed  · low: agy-acc1 12%
```

Status line: `SO ▶2 ◆1 ⚠1` with only non-zero counts; `SO down` when the supervisor is down and the session has active runs; cleared when the session has no runs in the window.

Toasts: one when a run newly gets `attention` (`SO: e3-core needs review`, `SO: e4-sales is stuck`, `SO: e4-e2e failed`) or the orchestrator lane turns `user`. Never on the first poll after load.

## Polling

- `session.start`: find the client pid with `$.process.run(['sh', '-c', 'echo $PPID'])`, read the theme, register `/so-monitor`, poll once, then start `$.clock.every`.
- Every 2 s while the session has active runs; every 10 s when it has none or the supervisor is down.
- One poll at a time (an in-flight guard). Argv: `superorchestrator monitor --json --client-pid <pid> --cwd <session cwd> --window <minutes>`, timeout 10 s.
- When the binary cannot start, the pane says `superorchestrator is not on PATH` and polling stops until `/so-monitor` runs again.
- `userConfig`: `monitor` (boolean, default true: auto-open the pane where it docks) and `band` (boolean, default true).

## Tasks

T1 runs in parallel with T2. T3 starts after T2 lands. T1 and T2 touch no common file.

Type-checking the mod: the API declarations for this Claude Code build are at
`/home/dat/.cache/tmp/claude-1000/-home-dat-dev-superorchestrator/eb3964ce-9e13-482c-85e7-92ae8231a92b/scratchpad/mod-types/claude-code.d.ts`.
Copy it to `plugin/.claude-plugin/types/claude-code/index.d.ts` (ignored by git) before running `tsc`.

### T1 `superorchestrator monitor` command (Rust)

- Goal: the command and JSON contract above.
- Files: `crates/so-core/src/procinfo.rs` (`parent_pid`), `crates/superorchestrator/src/cmd/monitor.rs` (new), `crates/superorchestrator/src/cli.rs`, `crates/superorchestrator/src/cmd/mod.rs`, `crates/superorchestrator/src/main.rs`, `crates/superorchestrator/tests/monitor.rs` (new), `SPEC.md` (one line in the section 15 command list), `CHANGELOG.md`.
- Reuse: `cmd/status.rs` `supervisor_status()` for the supervisor block, `views.rs` and the store's headroom and flag queries for accounts, the store's run and event queries for runs.
- Tests: segments from a seeded event sequence (including a clipped first segment and an open last one); every `attention` rule; marks mapping; open question detection; matching by id prefix, by cwd, and no match; the window filter for finished runs; tree order; supervisor not running gives `running: false` with null fields; percentages rounded to integers; exact top-level and per-run keys. Unit test for `parent_pid` of the current process.
- Acceptance: `cargo fmt --check`, `cargo clippy -p so-core -p superorchestrator --all-targets -- -D warnings`, `cargo test -p so-core procinfo`, `cargo test -p superorchestrator --test monitor`.
- Out of scope: web API or UI changes, any change to `status`, starting the supervisor, the mod.

### T2 Mod contract and pure logic (TypeScript)

- Goal: the types and pure functions the mod draws from, fully tested, with no hooks yet.
- Files: `plugin/types/index.d.ts` (snapshot types mirroring the JSON contract; lane, tone and theme types; `interface PluginState` for `superorchestrator`), `plugin/hooks/model.ts`, `plugin/tests/model.test.ts`, `plugin/tests/fixtures.ts` (snapshot fixtures), `plugin/tsconfig.json` (the hooks-module tsconfig from the API header), `plugin/.gitignore` (`.claude-plugin/types/`), `plugin/.claude-plugin/plugin.json` (add `"types": "./types/index.d.ts"`, nothing else).
- `PluginState['superorchestrator']` keys: `snapshot` (Snapshot or null), `fetch` (`{ status: 'idle' | 'ok' | 'down' | 'missing' | 'error'; at: number; error: string | null }`), `orchestrator` and `supervisor` (lane segments `{ tone, from, to }` in epoch ms, `to` null while open), `theme` (`{ mode, ansi, daltonized }`), `windowMin` (number), `expanded` (run ids), `clientPid` (number or null).
- `model.ts` exports: `parseSnapshot(text)` (defensive; null on any shape error), `toneOfRunState(state)`, `TONES` (tone to theme key, bar glyph, end mark), `themeInfo(value)`, `attentionCounts(snapshot, orchestratorLane)`, `quotaRows(accounts, maxRows)` (sorted and folded as in Layout), `barCells(pct, width)`, `laneRuns(segments, window, width)` (terminal: adjacent cells of one tone merged into `{ text, tone }` runs; a segment shorter than a cell still gets one cell when it is the last one), `laneSpans(segments, window)` (desktop: `{ tone, pct }` with gaps as `null` tone), `runRows(runs)` (tree prefixes), `formatDuration(ms)`, `axisLabels(windowMin, width)`, `bandText(counts, quota)`, `statusText(counts, supervisor)`, `newAttention(prev, next)` (the transitions that raise toasts).
- Tests (`claude plugin test`): each export, including empty and malformed snapshots, narrow widths (20 columns), daltonized and light themes.
- If `claude plugin test` needs a hooks module to load the plugin, add `plugin/hooks/hooks.json` and a `plugin/hooks/register.tsx` whose `register` does nothing yet.
- Acceptance: copy the API declarations as described above, then `/home/dat/dev/superorchestrator/web/node_modules/.bin/tsc -p plugin`, `claude plugin validate plugin --strict`, `claude plugin test plugin`.
- Out of scope: hooks, rendering, polling, Rust.

### T3 Mod hooks and rendering (TypeScript)

- Goal: the running mod as described in Lanes, Theme and surface detection, Layout and Polling, built on T2's `model.ts`.
- Files: `plugin/hooks/register.tsx`, `plugin/hooks/hooks.json`, `plugin/hooks/render.tsx` (terminal, desktop/vscode and mobile trees), `plugin/.claude-plugin/plugin.json` (`userConfig` `monitor` and `band`), `plugin/README.md` (what the monitor shows, `/so-monitor`, the two options, that it needs `superorchestrator` 0.1 or later on `PATH`), `plugin/tests/monitor.test.tsx`.
- Tests (`claude plugin test`, answering `process.run` with fixture snapshots from a hook beneath the plugin, clock mocked): the pane on `terminal` and `desktop` shows header, quota, counts and one row per run; pressing a run row shows its detail line; the band is drawn only with attention; the status text; a toast on a new attention and none on the first poll; the orchestrator lane turns `user` on an `AskUserQuestion` call; `missing` state when `process.run` rejects; light theme changes the empty-track color.
- Acceptance: as T2, plus `claude plugin validate plugin --strict` reports the `Pane` and `AbovePrompt` render hooks and the `/so-monitor` command.
- Out of scope: Rust changes, the web UI, actions on runs (accept, reject, cancel) from the pane.

## After the tasks

Final code-quality review of `main..feat/monitor-mod`, `cargo test --workspace` in the feature checkout, then a manual check in a live session: load `plugin/` with `CLAUDE_CODE_PLUGIN_DIRS` or the session's mods folder and confirm the pane, band and status line with a real run.
