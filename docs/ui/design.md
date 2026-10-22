# Web UI design

Status: design for SPEC section 14 (milestone M6). No code here. Terms follow [CONTEXT.md](../../CONTEXT.md).

Stack (already in `web/`): Vite, React 19, TypeScript, Tailwind v4, shadcn/ui (radix-nova, lucide icons, `@/` alias), TanStack Query, React Router. Components in `web/src/components/ui` today: `button`, `card`, `badge`. Everything else below is added with `pnpm dlx shadcn@latest add <name>`. New runtime dependencies: `recharts` (through the shadcn `chart` component), `@tanstack/react-virtual`, `sonner`, `msw` and `@playwright/test` (dev). No state library beyond TanStack Query; local UI state uses `useState` and URL search params.

The frontend talks only to `/api/v1` (SPEC 14). It never reads the database, socket or credential homes.

## 1. Information architecture

### 1.1 App shell

```
+--------------------------------------------------------------------------+
| [=] Superorchestrator   (supervisor: ● up 3h)   cap 4/8 [███░░]   [☾/☀]  |  header
+-----------+--------------------------------------------------------------+
| Dashboard |                                                              |
| Sessions  |                      <Outlet/>  (page)                       |
| Accounts  |                                                              |
| Choices   |                                                              |
| Agents    |                                                              |
| Workflows |                                                              |
| Settings  |                                                              |
+-----------+--------------------------------------------------------------+
```

- Sidebar: shadcn `Sidebar` (`SidebarProvider`, `SidebarMenu`, `SidebarMenuButton` with `NavLink`). Collapsible to icons at `lg`, off-canvas `Sheet` below `md` opened by `SidebarTrigger`. `Session timeline` is not a nav item; it is reached from Sessions and from Dashboard live runs.
- Header (`AppHeader`):
  - Supervisor status pill: `Badge` with a dot. Green "up" with uptime from `GET /status`; amber "reconnecting" when the SSE stream is down; red "unreachable" when `/status` fails twice. Tooltip shows pid, version.
  - Global cap usage: `Progress` plus text `active_runs / global cap`, from `GET /status` (`StatusResult.active_runs`) and `GET /config` (`caps.global`). Turns amber at 80 percent, red at 100 percent.
  - Theme toggle: `DropdownMenu` (light, dark, system). Persisted in `localStorage` (wrapped in try/catch), applied as a `dark` class on `<html>`.
  - `Command` palette (ctrl/cmd+K): jump to a page, a session or a run id.
- `Sonner` `Toaster` mounted once in the shell for action results and errors.

### 1.2 Routes

| Path | Page | Component |
|---|---|---|
| `/` | redirect to `/dashboard` | |
| `/dashboard` | Dashboard | `features/dashboard/DashboardPage` |
| `/sessions` | Sessions list | `features/sessions/SessionsPage` |
| `/sessions/:sessionId` | Session timeline | `features/timeline/TimelinePage` |
| `/sessions/:sessionId/runs/:runId` | Same page with the run panel open (deep link) | `features/timeline/TimelinePage` |
| `/accounts` | Accounts | `features/accounts/AccountsPage` |
| `/choices` | Choices and routing | `features/routing/RoutingPage` |
| `/agents` | Agents | `features/agents/AgentsPage` |
| `/agents/:name` | Agent editor | `features/agents/AgentEditor` |
| `/workflows` | Workflows | `features/workflows/WorkflowsPage` |
| `/workflows/:id` | Workflow run detail | `features/workflows/WorkflowRunPage` |
| `/settings` | Settings | `features/settings/SettingsPage` |
| `/login` | Token prompt (non-loopback only) | `features/auth/TokenPage` |
| `*` | Not found | `NotFoundPage` |

Pages are lazy-loaded (`React.lazy`) so the timeline code and `recharts` stay out of the initial bundle.

## 2. API (`/api/v1`)

Conventions:
- JSON, snake_case, same field names as `crates/so-core/src/proto.rs`. Types are named after the Rust types.
- Errors: HTTP status plus `{ "error": { "code": number, "message": string, "data"?: any } }`, where `code` is the JSON-RPC code from `proto.rs` (`NOT_FOUND` -32003 gives 404, `CAP_EXCEEDED` -32002 gives 409, `INVALID_PARAMS` -32602 gives 422, `UNAVAILABLE` -32004 gives 503, `DEPTH_LIMIT` -32001 gives 409). Typed in the client as `ApiError`.
- Auth: loopback needs nothing. Non-loopback requires the HttpOnly cookie set by `POST /api/v1/auth` (section 12).
- Lists are paginated with `?limit=&cursor=`; response `{ items: T[], next_cursor: string | null }`.
- The handlers call the same supervisor methods as the CLI and MCP tools, so each endpoint names its CLI twin.

### 2.1 Endpoint list

| Method and path | Request | Response | CLI twin |
|---|---|---|---|
| `GET /status` | | `StatusResult` plus `caps: { global: number }` | `status` |
| `GET /dashboard` | | `DashboardView` (below) | `status`, `account list`, `runs list` |
| `GET /usage/daily?days=30` | | `DailyUsage[]` (`{ day, input_tokens, output_tokens, cost, by_account: Record<string, number> }`) | `status --usage` |
| `GET /harnesses` | | `HarnessesView` | `list_harnesses` |
| `GET /sessions?status=&limit=&cursor=` | filter `active\|escalated\|failed` | `Page<SessionSummary>` | `runs list` |
| `GET /sessions/:id` | | `SessionDetail` = `SessionSummary` plus `runs: RunView[]` (no events) | `runs show` |
| `GET /sessions/:id/events?after=&limit=` | cursor is the last event id | `Page<Event>`, ordered by `(ts, id)`. The timeline loads all pages (10 000 events is 20 pages of 500). | `runs watch` |
| `GET /runs/:id` | | `RunView` | `get_run` |
| `GET /runs/:id/events?after=&type=&limit=` | | `Page<Event>` | `runs show --events` |
| `GET /runs/:id/transcript?offset=&limit=` | byte offset into `transcript.jsonl` | `TranscriptPage` (`{ lines: TranscriptLine[], next_offset: number \| null, total_bytes }`) | `runs show --transcript` |
| `GET /runs/:id/route` | | `RouteResult` (from the `route_decision` event) | |
| `GET /runs/:id/messages` | | `MessageView[]` | `read_messages` |
| `POST /runs/:id/messages` | `SendMessageParams` minus `run` | `AckResult` | `send_message` |
| `POST /runs/:id/cancel` | | `AckResult` | `cancel_agent` |
| `POST /runs/:id/accept` | | `AckResult` | `accept_run` |
| `POST /runs/:id/reject` | `{ feedback: string }` | `AckResult` | `reject_run` |
| `POST /runs/:id/join` | | `JoinResult` redacted: `{ argv, cwd, env_names }`. Env values are never returned to the browser. | `join` |
| `POST /route` | `RouteParams` | `RouteResult` | `route` |
| `GET /routing/decisions?limit=&cursor=` | | `Page<RouteDecisionRow>` (`{ ts, run_id?, prompt_excerpt, result: RouteResult }`) | `route log` |
| `GET /accounts` | | `AccountView[]` (extended, see open questions) | `account list` |
| `POST /accounts` | `{ harness, name, home? }` | `AccountView` | `account add` |
| `POST /accounts/import` | `{ name, harness, home }` | `AccountView` | `account import` |
| `GET /accounts/scan` | | `ImportCandidate[]` (`{ harness, name, home }`) | `account import --scan` |
| `PATCH /accounts/:name` | `{ cap?, priority?, enabled?, pools? }` | `AccountView` | `account enable\|disable`, config edit |
| `DELETE /accounts/:name?purge=` | | `AckResult` | `account remove` |
| `POST /accounts/:name/flag` | `{ pool?, until?: string, hours?: number, clear?: boolean, reason?: string }` | `AccountView` | `account flag` |
| `POST /accounts/:name/login` | | `LoginStart` (below) | `account login` |
| `GET /accounts/:name/login` | | `LoginStatus` (below), the poll target | |
| `GET /choices` | | `Choice[]` plus `routing_instruction: string` | `models`, config |
| `PUT /choices` | `{ choices: Choice[], routing_instruction: string }` | same | config edit |
| `GET /agents` | | `AgentsView` | `agent list` |
| `GET /agents/:name` | | `{ name, description, path, source: string }` (raw `AGENT.md`) | `agent show` |
| `PUT /agents/:name` | `{ source: string }` | `{ ok, warnings: string[] }` | `agent edit` |
| `DELETE /agents/:name` | | `AckResult` | `agent remove` |
| `GET /workflows` | | `WorkflowSummary[]` (saved) | |
| `GET /workflows/runs?limit=&cursor=` | | `Page<WorkflowRunView>` | |
| `GET /workflows/runs/:id` | | `WorkflowStatusResult` plus `steps: WorkflowStep[]` | `workflow_status` |
| `POST /workflows/:name/run` | `{ args?: unknown }` | `RunWorkflowResult` | `run_workflow` |
| `GET /config` | | `ConfigView`, secrets replaced by `{ source: "env"\|"file"\|"unset", name?: string }` | `config get` |
| `PATCH /config` | partial `ConfigView` | `ConfigView` | `config set` |
| `POST /auth` | `{ token }` | 204 and `Set-Cookie` | |
| `GET /stream` | `Accept: text/event-stream`, `?session=` optional filter | SSE (section 2.3) | `runs watch` |

Types not in `proto.rs` that the supervisor adds in `so-core` (the API task generates them from the same `schemars` schemas): `SessionSummary` (`{ id, kind: "orchestrator"|"interactive", client, cwd, account?, started_at, ended_at?, status: "active"|"escalated"|"failed"|"done", run_count, cost }`), `DashboardView` (`{ status: StatusResult, caps, accounts: AccountView[], live_runs: RunView[], escalations: Event[], daily: DailyUsage[] }`), `LoginStart`, `LoginStatus`, `ConfigView`, `TranscriptPage`, `WorkflowSummary`, `WorkflowRunView`, `WorkflowStep`.

`LoginStart` = `{ mode: "device_auth", url: string, code: string, expires_at: string } | { mode: "manual", command: string, cwd?: string }`.
`LoginStatus` = `{ signed_in: SignInState, mode: "device_auth"|"manual", state: "pending"|"done"|"expired"|"failed", message?: string }`.

### 2.2 Query keys

```
["status"] ["dashboard"] ["usage","daily",days] ["harnesses"] ["accounts"] ["accounts","scan"]
["accounts",name,"login"] ["sessions",filter] ["session",id] ["session",id,"events"]
["run",id] ["run",id,"events"] ["run",id,"transcript"] ["run",id,"route"] ["run",id,"messages"]
["choices"] ["routing","decisions"] ["agents"] ["agent",name] ["workflows"] ["workflow-runs"]
["workflow-run",id] ["config"]
```

### 2.3 SSE `/api/v1/stream`

One `EventSource` for the whole app, opened by `useEventStream()` in the shell. Each message: `event: <name>`, `id: <monotonic seq>`, `data: <json>`. The browser reconnects with `Last-Event-ID`; the supervisor replays from that sequence, or sends `event: resync` if the gap is too old.

| SSE event | `data` | Used for |
|---|---|---|
| `event` | `Event` (the SPEC 12 shape) | Every appended event. Drives the timeline append and the invalidations below. |
| `run` | `RunView` without `recent_events` | State or usage change of a run; patches `["run",id]` in the cache. |
| `session` | `SessionSummary` | Session start, end, status change. |
| `headroom` | `Headroom` | Patches `["accounts"]` and `["harnesses"]`. |
| `account` | `AccountView` | Sign-in, enable, flag or cap changed; also finishes login polling early. |
| `hello` | `{ seq, server_time }` | First message, used to detect restarts. |
| `resync` | `{}` | Invalidate everything and refetch. |

Invalidation map (done in one place, `api/stream.ts`, as a pure `invalidationsFor(event) => QueryKey[]`):

| Trigger | Invalidates |
|---|---|
| `event` of any type | append to `["session",sid,"events"]` cache (no refetch); `["dashboard"]` throttled to 1 per 2 s |
| `state_changed`, `run_started`, `run_finished`, `run_queued` | `["run",rid]`, `["sessions"]`, `["status"]` |
| `usage` | `["run",rid]`, `["usage","daily"]` (throttled 10 s) |
| `route_decision` | `["run",rid,"route"]`, `["routing","decisions"]` |
| `message_sent`, `question_asked`, `question_answered` | `["run",rid,"messages"]` |
| `escalated`, `stall_detected` | `["dashboard"]` (escalations list) |
| `worktree_created`, `worktree_removed` | `["run",rid]` |
| `headroom`, `account` | `["accounts"]`, `["harnesses"]`, `["dashboard"]` |
| `resync` or reconnect after a gap | all |

When the stream is down the app falls back to `refetchInterval: 5000` on `dashboard`, `sessions` and the open run, and shows the amber header pill. Transcript is not pushed; the open Transcript tab polls `?offset=<next_offset>` every 2 s while the run is active.

## 3. Pages

All pages use a `PageHeader` (title, description, actions) and, for data pages, the same three states: `Skeleton` while loading, `Alert variant="destructive"` with a Retry `Button` on error, and an `Empty` block (icon, sentence, primary action) when empty.

### 3.1 Dashboard

Purpose: one screen that says whether the system has capacity and whether anything needs attention.

```
+-------------------------------------------------------------------------+
| Dashboard                                                               |
| [Cap 4/8 ███░░] [Live runs 4] [Escalations 1] [Cost today $3.21]        |  4 stat Cards
| Accounts                                                                |
| Account      Harness  Sign-in  Pool   5h headroom     Weekly   Runs     |
| codex-acc1   codex    ●        main   ███████░ 72%    ██████░ 61%  1/2  |
| agy-acc1     agy      ●        gemini ████░░░░ 40%    ██░░░░░ 22%  0/1  |
|                                claude ██████░░ 80%    ...               |
| Live runs                          | Recent escalations                  |
| ▸ refactor-api   running  codex-acc1  2m | ⚠ run-7f stalled 3 nudges 12:04 |
| ▸ write-tests    waiting  agy-acc1    5m |                                  |
| Cost and tokens per day (30d)  [Cost|Tokens] toggle                     |
| ▂▃▅▇▅▃▂▄▆  stacked bars per account                                    |
+-------------------------------------------------------------------------+
```

Components: `Card` (stats, sections), `Table` (accounts), `Progress` (headroom; label percent, reset time in a `Tooltip`), `Badge` (sign-in state, run state), `Tabs` (cost or tokens toggle), `Chart` (recharts `BarChart`, stacked per account), `Skeleton`, `Alert`, `ScrollArea` (live runs list), `Tooltip`.

Data: `GET /dashboard` (single request for first paint), `GET /usage/daily` for the chart range. Rows link to `/accounts`, `/sessions/:sid/runs/:rid`.
Live: `run`, `account`, `headroom`, `session` patch the cache; `escalated`, `stall_detected` refresh the list. Headroom bars colour: green above 30 percent, amber 10 to 30, red below 10 or `HeadroomStatus` `exhausted`; `unknown` renders a hatched empty bar with "unknown". Never colour alone: the percent text is always shown.

### 3.2 Sessions

Purpose: find a session (an orchestrator's tree of runs, or an interactive wrapper launch) and open its timeline.

```
+-------------------------------------------------------------------------+
| Sessions        [All|Active|Escalated|Failed]   [search by client/cwd ] |
| Started      Kind          Client       Runs  Status      Cost          |
| 12:01 today  orchestrator  claude-code  5     ● active    $0.84    ▸    |
| 11:20 today  interactive   codex-acc2   0     done        $0.10    ▸    |
+-------------------------------------------------------------------------+
|                                           [Load more]                   |
```

Components: `ToggleGroup` (filter, mirrored to `?status=`), `Input`, `Table`, `Badge`, `Button` (load more, `useInfiniteQuery`), `Skeleton`, `Empty`.
Data: `GET /sessions?status=`. Live: `session` event patches rows; new sessions are prepended when they match the filter.

### 3.3 Session timeline

Purpose: see who ran what, when, on which account, and why it stalled or failed over. Detailed spec in section 4.

```
+-------------------------------------------------------------------------+
| ◂ Sessions / claude-code 12:01        [Fit][−][+]  [Live ●] [Legend]    |
|                          12:01      12:05      12:10      12:15   now    |
| orchestrator  ░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░   |
|  ├ refactor    ▓▓▓▓▓▓▓▓▓▓▓|▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓|▒▒▒|▓▓▓▓  ┆ ┆  ┆ ┆       |
|  │   └ tests              ▓▓▓▓▓▓▓▓▓⚠▓▓▓                                 |
|  └ docs                       ▒▒▒▒▒▒▒▒▒▒▒▓▓▓▓▓▓▓✔                       |
| minimap  [██████████░░░░░░░░░░░░░░░░░░]                                 |
+-------------------------------------------------------------------------+
```

Components: custom SVG `GanttCanvas` inside `ScrollArea`; shadcn `Button`, `ToggleGroup`, `Slider` (zoom), `Popover` (legend), `Tooltip` (hover details), `Sheet` (run panel), `Tabs`, `Badge`, `Skeleton`, `Alert`.
Data: `GET /sessions/:id` (runs), `GET /sessions/:id/events` (all pages, then live). Live: SSE `event` appended directly, see 4.7.

### 3.4 Accounts

Purpose: manage logins. Detailed login flow in section 5.

```
+-------------------------------------------------------------------------+
| Accounts                          [Scan & import] [+ Add account]       |
| Name   Harness Sign-in  Enabled Cap  Prio  Pools        Headroom   ⋯    |
| codex-acc1 codex ● in   [on]    2    1     main         5h ███ 72%      |
| agy-acc1   agy   ○ out  [on]    1    2     gemini,claude  ...   [Sign in]|
+-------------------------------------------------------------------------+
```

Components: `Table`, `Switch` (enable), `Badge`, `Progress`, `DropdownMenu` (row actions: Edit, Flag cooldown, Sign in, Remove), `Dialog` (add, import, login), `AlertDialog` (remove, type the name to confirm), `Sheet` (edit cap, priority, pools: `Form`, `Input type=number`, `Checkbox` per pool), `Popover` plus `Calendar`/`Input` (flag until), `Select` (harness), `Sonner`.
Data: `GET /accounts`; mutations per table above. Live: `account`, `headroom` events.

### 3.5 Choices and routing

Purpose: edit the approved Choices and the routing instruction, preview a Route, review past decisions.

```
+-------------------------------------------------------------------------+
| Choices & routing   [Choices] [Routing instruction] [Preview] [Log]     |
| Choices tab:                                                            |
| Id          Harness Model         Effort Roles        Enabled  Description|
| codex-high  codex   gpt-5.x       high   code,review  [on]     Hard ... |
|                                                       [+ Add choice]    |
| Preview tab:                                                            |
| Prompt [textarea]  Role [select] Pin: harness/account/model/effort      |
| [Run preview]                                                           |
|  Result: codex-high on codex-acc1  source: jev  confidence 0.82         |
|  codex-high  ████████ 0.62 | agy-pro ███ 0.21 | ...   Rationale: ...    |
+-------------------------------------------------------------------------+
```

Components: `Tabs`, `Table` with inline edit through `Dialog` + `Form` (`Input`, `Select` for harness, `Textarea` for description, `Switch` enabled, `Checkbox`/`ToggleGroup` for roles), `Textarea` (instruction, with a character count and Save/Revert buttons), `Progress`-style probability bars (shared `ProbabilityBars` component), `Badge` (source: jev, heuristic, pinned), `Table` + `Pagination` (log), `Sonner`.
Data: `GET/PUT /choices`, `POST /route`, `GET /routing/decisions`. Live: `route_decision` invalidates the log. Saving shows validation errors from 422 `data` next to fields.

### 3.6 Agents

Purpose: list, view and edit `AGENT.md` files.

Components: `Table` (name, description, source path, harness overrides), `Button` New, editor page with a monospace `Textarea` (v1: no code editor dependency) for the file, a read-only `Card` showing parsed frontmatter (from `warnings` after save), `AlertDialog` for delete, `Sonner`.
Data: `GET /agents`, `GET/PUT/DELETE /agents/:name`. Live: none; refetch on window focus.

```
+---------------------------------------------------------------+
| Agents                                          [+ New agent] |
| Name        Description                       Path         ⋯  |
| reviewer    Reviews diffs for bugs            ~/.../reviewer.md|
+---------------------------------------------------------------+
```

### 3.7 Workflows

Purpose: saved workflows, their runs and step status.

```
+---------------------------------------------------------------+
| Workflows        [Saved] [Runs]                                |
| Runs: id  name  state  started  duration  session    ▸        |
| Detail: steps list (name, state badge, run link, duration) +   |
|         result JSON (collapsible <pre>) + [Run again]          |
+---------------------------------------------------------------+
```

Components: `Tabs`, `Table`, `Badge`, `Collapsible` + `<pre>` for JSON, `Dialog` + `Textarea` (args JSON) for Run, `Skeleton`, `Empty`.
Data: `GET /workflows`, `GET /workflows/runs`, `GET /workflows/runs/:id`, `POST /workflows/:name/run`. Live: `session` and `event` for the workflow's session invalidate `["workflow-run",id]`.

### 3.8 Settings

Purpose: edit `config.toml` fields. Sections as `Tabs` or an `Accordion`: General (caps, stall_ms), Notifications, Worktree defaults, Routing (min_confidence, min_margin), OpenRouter key source, UI (bind, port; changing shows "restart required" `Alert`).

Components: `Form` with `Input`, `Select`, `Switch`, `Textarea`, `Separator`, `Alert`, `Button`, `Sonner`.
OpenRouter key: shows only `source` (env var name, file path, or unset) as a `Badge` and a "Key set: yes/no" line. There is no input that displays a stored value. To change it the user edits the source name (env var name) or path; a pasted value is not supported in v1.
Data: `GET/PATCH /config`. Live: none.

## 4. Session timeline Gantt (detailed)

### 4.1 Data model

```ts
type RunNode = { run: RunView; children: RunNode[]; lane: number };
type Segment =
  | { kind: "state"; runId: string; state: RunState; start: number; end: number | null; open: boolean }
  | { kind: "attempt"; runId: string; account: string; start: number; end: number | null;
      outcome: "ok" | "failover" | "failed" | "running"; reason?: string };
type Tick   = { kind: "tool"; runId: string; at: number; name: string; ok: boolean | null };
type Arrow  = { kind: "message"; from: string; to: string; at: number; msgKind: MessageKind };
type Marker = { kind: "stall" | "nudge" | "escalated" | "question" | "check" | "review"; runId: string; at: number; label: string; ok?: boolean };
type RunLane = { runId: string; segments: Segment[]; attempts: Segment[]; ticks: Tick[]; markers: Marker[] };
type TimelineModel = { lanes: RunLane[]; arrows: Arrow[]; tMin: number; tMax: number; lastSeq: number };
```
Times are epoch milliseconds.

### 4.2 Derivation (pure, unit-tested)

`deriveTimeline(runs: RunView[], events: Event[], now: number): TimelineModel` in `features/timeline/derive.ts`, no React, no `Date.now()` (callers pass `now`). A companion `applyEvent(model, event, now)` returns a new model for live append; `deriveTimeline(runs, events)` must equal folding `applyEvent` over the same events (tested as a property).

Algorithm, events sorted by `(ts, id)` once:
1. Lane order: depth-first over `parent_run` links, children ordered by `created_at`; the orchestrator lane is a synthetic lane with `runId = "orchestrator"` at the top (events with `run_id == null`, user joins, message arrows from the orchestrator).
2. Per run keep `cur: { state, start }`. `run_queued` sets `queued`. `state_changed{from,to}` closes `cur` at `ts` (end) and opens `{to, ts}`. `run_started` opens `starting` if no state is open. `run_finished{status}` closes the open segment and ends the lane (`open=false`). The last open segment gets `end = now` (`open: true`) while the run is not terminal. If a `state_changed.from` disagrees with `cur.state`, trust the event, insert nothing, and count a `warnings` entry (surfaced in dev console only).
3. Attempts: `attempt_started{account}` opens an attempt; `failover{from,to,reason}` closes the open attempt with `outcome:"failover"` and `reason`; `run_finished` closes it with `ok` (succeeded) or `failed`. Attempts render as a thin bar under the state bar, labelled with the account.
4. Ticks: `tool_call_started` creates a tick; a later `tool_call_finished` with the same `name` for that run (FIFO match) sets `ok`. Unmatched finishes create a tick with `ok` set. Ticks closer than 2 px at the current zoom are clustered at render time into one tick with a count (clustering is in the renderer from the same data, not in derive).
5. Markers: `stall_detected` (stall), `nudge_sent`, `escalated`, `question_asked` (with `question_answered` pairing to a span), `check_result{ok}`, `review_accepted`, `review_rejected`.
6. Arrows: `message_sent{to, kind}` from the event's `run_id` (or orchestrator) to `to` at `ts`.
7. `tMin` is the earliest event or `created_at`; `tMax` is `now` while any run is active, otherwise the latest `finished_at`.
8. Complexity O(n) after the sort; the sort is skipped for appends because SSE delivers in order, and events with a `ts` earlier than `lastSeq`'s `ts` trigger a full re-derive.

Colour is a pure function of `RunState`; `usage`, `assistant_message`, `progress` and `route_decision` do not draw on the chart (they appear in the Events tab).

### 4.3 Colours

Defined as CSS variables on `:root` and `.dark` in `index.css` (`--state-queued` and so on) and used with `fill="var(--state-x)"`. Each state also has a distinct SVG pattern or shape so state is not colour-only.

| State | Light | Dark | Pattern/shape cue |
|---|---|---|---|
| queued | `#94a3b8` | `#64748b` | dotted fill |
| starting | `#38bdf8` | `#0ea5e9` | diagonal hatch |
| running | `#16a34a` | `#22c55e` | solid |
| waiting | `#2563eb` | `#60a5fa` | solid with light inner stripe |
| stalled | `#d97706` | `#fbbf24` | cross hatch |
| escalated | `#dc2626` | `#f87171` | cross hatch plus marker |
| pending_review | `#9333ea` | `#c084fc` | solid with dashed outline |
| succeeded | `#0f766e` | `#2dd4bf` | solid, check glyph at end |
| failed | `#991b1b` | `#ef4444` | solid, x glyph at end |
| cancelled | `#52525b` | `#a1a1aa` | strikethrough line |

Contrast requirement: segment fill against the lane background at least 3:1 (WCAG 1.4.11 non-text); text on segments (the state label when wide enough) is white or near-black chosen per fill at least 4.5:1. A Vitest test computes the contrast of each pair in both themes and fails below the thresholds. The legend (`Popover`, also pinned in the header on `xl`) lists every state with its swatch and pattern.

### 4.4 Rendering: SVG, not canvas

Decision: SVG, rendered only for the visible window.

Justification:
- 10 000 events does not mean 10 000 shapes. Segments are bounded by state changes (a run has dozens), so the heavy element is tool ticks. After clustering by pixel at the current zoom and culling to the visible time window, the DOM holds at most about 150 ticks per lane plus under 100 segments per lane, a few thousand nodes in the worst case, which React and SVG handle at 60 fps.
- SVG gives native hit testing, CSS variable theming (dark mode with no redraw logic), text, focus and ARIA for each lane and marker, and crisp output at any device pixel ratio. Canvas would need hand-written hit testing, accessibility mirroring and a theme repaint.
- Escape hatch: the renderer takes only `TimelineModel` and a viewport, so a canvas renderer can replace `GanttCanvas` later without touching derivation. Switch criterion: if the Playwright perf test (4.8) cannot hold 30 fps pan with 10 000 events.

Layout: a fixed left gutter (lane labels, `HTML`, sticky, indent by depth, state badge and account) and a scrolling SVG for the plot. Lane height 32 px; state bar 14 px; attempt bar 4 px below it; ticks 8 px above; markers as 12 px glyphs on the bar. Time axis at the top, sticky, ticks chosen by a "nice step" function (1 s, 5 s, 15 s, 1 m, 5 m, 15 m, 1 h) giving 6 to 12 labels.

### 4.5 Zoom, pan, virtualisation

- Viewport state `{ t0, t1 }` in ms. Zoom: ctrl/cmd+wheel or `+`/`-` keys or the `Slider`, anchored at the pointer. Pan: drag on empty plot area, shift+wheel, or arrow keys. `Fit` sets the viewport to `[tMin, tMax]`. Minimum span 1 s, maximum 2x the session.
- Minimap (SVG strip showing lane density and the current viewport rectangle) is draggable.
- Vertical virtualisation: lanes via `@tanstack/react-virtual` (only when lanes exceed 60; below that render all).
- Horizontal/time culling: per lane, binary search over the time-sorted `segments`, `ticks`, `markers` for `[t0 - pad, t1 + pad]`. Ticks are clustered into buckets of 3 px (`bucket = floor(x / 3)`), each rendered as one `<line>`; a bucket with more than one tick shows its count on hover and zooming in splits it.
- Arrows: drawn only if both endpoints' lanes and the time are within the viewport; curved `path` from sender lane to receiver lane with an arrow head; capped at 200 drawn, others summarised as "+N more" in a corner chip. Hover highlights both lanes.
- The plot is wrapped in `React.memo` per lane keyed by lane identity and viewport; the model update on live append produces new objects only for the touched lane.

### 4.6 Interactions and accessibility

- Click a lane label or any of its bar: select the run, opens the run panel, route becomes `/sessions/:sid/runs/:rid`. Esc closes.
- Hover: `Tooltip` with state, start, duration, account, attempt reason; for markers, the label; for ticks, tool name and ok.
- Keyboard: the plot has `role="application"` with an `aria-label`; lanes are a `role="list"` of focusable `role="listitem"` rows (tab order = lane order); Enter opens the panel; left/right move between segments in the focused lane, announced through `aria-live="polite"` ("running, 12:01 to 12:04, codex-acc1"). A visually hidden table summary of the same data (`<table class="sr-only">`) is present for screen readers.
- `prefers-reduced-motion`: no animated "now" edge; live follow becomes a button.

### 4.7 Live append

- The page loads runs and events once, derives the model, and stores it in a `useReducer` (not TanStack Query state; the events cache stores only the raw array).
- `useEventStream` pushes SSE `event` payloads into a ref buffer; a `requestAnimationFrame` flush (at most 1 per frame, or every 100 ms when the tab is hidden) calls `applyEvent` for each, and appends to the raw events cache with `queryClient.setQueryData`.
- Events whose `session_id` differs are ignored. Unknown run ids (a run spawned after load) trigger a refetch of `["session",id]` to get the new `RunView`, and the lane appears when it arrives.
- "Live" toggle: when on, the viewport right edge follows `now` (ticked by a 1 s timer that only moves open segment ends and the axis). Panning or zooming turns it off; a "Jump to live" button restores it.
- On `resync` or reconnect with a gap: refetch events with `after=<last id>`; if rejected, full reload.

### 4.8 Performance target (M9: 10 000 events per session)

- Initial load: events fetched in pages of 500 in parallel after the first page returns `next_cursor`s (server cursor is by id, so pages are fetched sequentially; the UI renders after the first page and progressively fills, showing a progress bar). Derive of 10 000 events in under 50 ms (Vitest benchmark guard at 200 ms in CI).
- Interaction: pan and zoom render under 16 ms per frame with 10 000 events, 30 runs (Playwright trace assertion later).
- Memory: events kept as the API objects, no copies; derive output indexes by run id.
- Run panel Events tab: virtualised list (`@tanstack/react-virtual`), 10 000 rows.

### 4.9 Run panel (Sheet)

`Sheet` from the right (width 560 px, full width on mobile) with header: run name or id, `Badge` for state, harness / account / model, duration, cost. `Tabs`:

| Tab | Content | Data |
|---|---|---|
| Brief | The prompt/brief and spawn options (agent, isolation, review, checks, timeout) | `RunView`; brief from the `run_queued` event data |
| Route | `ProbabilityBars` (every entry of `probabilities`, selected choice highlighted, bars sorted descending, numeric percent label), `source` badge, `confidence`, `rationale` text, `alternatives` list with account and score, usage (tokens, cost) of the decision | `GET /runs/:id/route` |
| Events | Virtualised list (`ScrollArea` + react-virtual), filter by type group (lifecycle, activity, messages, stalls) with `ToggleGroup`, each row `time, type badge, summary`, click expands JSON | `["run",id,"events"]`, shares the session cache |
| Transcript | Virtualised, variable-height rows (`measureElement`), normalised lines: role-coloured; tool calls collapsed `Collapsible`; "Load older" at top, auto-scroll to bottom when at bottom and the run is active; search `Input` (client-side over loaded lines) | `GET /runs/:id/transcript` |
| Result | `result` JSON rendered as a collapsible tree or `<pre>`; `error_kind` `Alert` when failed; schema validation note when `output_schema` was set | `RunView.result` |
| Worktree | path, branch, cwd, isolation, copy buttons; check results (`check_result` events) as a list with ok badges | `RunView`, events |
| Join | The command line built from `argv` and `cwd` (`cd <cwd> && ...`), env variable names listed as "sets: CODEX_HOME" (never values), a `Button` "Copy command" (`navigator.clipboard`, toast "Copied"). The command is fetched with `POST /runs/:id/join` only when the user opens the tab or clicks Copy (it records `join_command_issued`). | `POST /runs/:id/join` |

Footer actions (`Button` row, enabled by state):
- Send message: opens inline `Textarea` + `Select` for `MessageKind` (message or nudge) + Send; `POST /runs/:id/messages`. Allowed for any run (a finished run is resumed; the button label says "Send and resume" when terminal).
- Cancel: `AlertDialog` ("cancels this run and its subtree"); enabled when active.
- Accept and Reject: only in `pending_review`. Reject opens a `Dialog` with a required feedback `Textarea`.
- All use optimistic disable plus `Sonner` result and invalidate `["run",id]`; failures show the error message from the API.

## 5. Accounts page and login flow

Add account `Dialog` (steps): 1. harness `Select` and name `Input` (validated `^[a-z0-9-]+$`, unique); optional home path under "Advanced". 2. `POST /accounts` creates the account (home, toml, wrapper) and returns it signed out. 3. Login step starts automatically.

Login step (`LoginDialog`, also opened from the row "Sign in"):
- `POST /accounts/:name/login` returns `LoginStart`.
- `device_auth` (codex): shows the URL as a link (`target="_blank" rel="noreferrer"`) with an Open button, and the one-time code in large monospace with a Copy button, plus "Expires in mm:ss" countdown from `expires_at`. Below: "Waiting for sign-in" with a spinner.
- `manual` (agy, opencode, claude, codex without device-auth): shows the command (`CommandBlock` with Copy) with the note "Run this in a terminal, then come back". Same waiting state.
- Polling: `GET /accounts/:name/login` every 2 s (TanStack Query `refetchInterval`), stopping when `state` is `done`, `expired` or `failed`, or the dialog closes; also ends early on the SSE `account` event with `signed_in`. On `done`: success `Alert`, toast, close. On `expired`/`failed`: error `Alert` with "Try again" which re-posts the login. The sign-in check is only that the auth file exists (SPEC 5.4); the UI says "signed in" accordingly, never shows file content.
- Import (`Scan & import` button): `GET /accounts/scan` fills a `Dialog` table with `Checkbox` rows (harness, name, home); confirm calls `POST /accounts/import` per row, with per-row status.
- Edit `Sheet`: cap (integer 1 to 16), priority (integer), pools (agy: gemini and claude families) as `Checkbox`es, enabled `Switch`. Flag cooldown `Dialog`: pool `Select` (all or one), duration `Select` (1 h, 5 h, 24 h, custom until) and clear; shows current flags from the account view.
- Remove `AlertDialog`: user must type the account name; checkbox "Also delete the credential home" (purge) off by default with a destructive warning.
- macOS rule (SPEC 6.4/5): the agy add action is disabled with a `Tooltip` when `GET /config` reports macOS and an agy account already exists.

## 6. Module structure (`web/src`)

```
web/src/
  main.tsx                 providers: QueryClientProvider, RouterProvider, ThemeProvider, Toaster
  App.tsx                  route table (lazy pages)
  index.css                tokens, --state-* colours, dark variant
  api/
    types.ts               hand-written TS mirror of proto types (below)
    client.ts              fetch wrapper: base URL, JSON, ApiError, 401 -> /login
    endpoints.ts           one typed function per endpoint of section 2.1
    queries.ts             query key factory + useXxx hooks (useAccounts, useRun, ...)
    stream.ts              useEventStream(), invalidationsFor(event) (pure), reconnect state
    msw/                   handlers.ts and fixtures.ts (used by tests and `pnpm dev:mock`)
  components/
    ui/                    shadcn generated (do not hand edit beyond tokens)
    layout/                AppShell, AppSidebar, AppHeader, PageHeader, ThemeToggle, SupervisorPill, CapMeter
    common/                StateBadge, HarnessBadge, HeadroomBar, ProbabilityBars, CommandBlock, CopyButton,
                           ErrorAlert, EmptyState, TableSkeleton, RelativeTime, Duration, JsonView, ConfirmDialog
  features/
    dashboard/             DashboardPage, StatCards, AccountsSummaryTable, LiveRuns, Escalations, UsageChart
    sessions/              SessionsPage, SessionsTable, filters.ts
    timeline/              TimelinePage, derive.ts, applyEvent.ts, viewport.ts, colors.ts,
                           GanttCanvas.tsx, LaneLabel.tsx, TimeAxis.tsx, Minimap.tsx, Legend.tsx, useTimelineModel.ts,
                           run-panel/ (RunPanel, BriefTab, RouteTab, EventsTab, TranscriptTab, ResultTab,
                                       WorktreeTab, JoinTab, RunActions)
    accounts/              AccountsPage, AccountsTable, AddAccountDialog, LoginDialog, ImportDialog,
                           EditAccountSheet, FlagDialog, RemoveAccountDialog
    routing/               RoutingPage, ChoicesTable, ChoiceDialog, InstructionEditor, RoutePreview, DecisionLog
    agents/                AgentsPage, AgentEditor
    workflows/             WorkflowsPage, WorkflowRunPage
    settings/              SettingsPage, sections/*
    auth/                  TokenPage
  lib/
    utils.ts               cn()
    format.ts              money, tokens, duration, percent
    time.ts                epoch helpers, nice-step axis ticks
    contrast.ts            WCAG contrast (test use and dev assertion)
    theme.ts               theme storage with try/catch
  test/                    setup.ts (jest-dom, MSW server), render.tsx (providers)
```

Rules: a `features/x` folder never imports another feature except `features/timeline/run-panel` which is also opened from the dashboard. Shared pieces go in `components/common`. Server state only in TanStack Query; no global stores.

### 6.1 TypeScript types (`api/types.ts`)

Hand-written, one-to-one with `proto.rs` and `types.rs` (the API task adds a CI check that diffs these against `schemars` JSON schema output, see T2.3).

```ts
export type Harness = "codex" | "agy" | "opencode" | "claude";
export type RunState = "queued" | "starting" | "running" | "waiting" | "stalled" | "escalated"
  | "pending_review" | "succeeded" | "failed" | "cancelled";
export type ErrorKind = "quota" | "rate_limit" | "auth" | "other";
export type Isolation = "worktree" | "none";
export type MessageKind = "message" | "progress" | "question" | "answer" | "nudge" | "result";
export type SignInState = "signed_in" | "signed_out" | "unknown";
export type HeadroomStatus = "ok" | "low" | "exhausted" | "unknown";
export type RouteSource = "jev" | "heuristic" | "pinned";
export type EventType = "run_queued" | "run_started" | "state_changed" | "run_finished" | "attempt_started"
  | "failover" | "route_decision" | "tool_call_started" | "tool_call_finished" | "assistant_message"
  | "progress" | "message_sent" | "question_asked" | "question_answered" | "usage" | "user_joined"
  | "user_left" | "join_command_issued" | "stall_detected" | "nudge_sent" | "escalated"
  | "worktree_created" | "worktree_removed" | "check_result" | "review_accepted" | "review_rejected";

export interface Event { id: string; session_id: string; run_id: string | null; ts: string; type: EventType; data: unknown }
export interface Headroom { account: string; pool: string; five_hour_remaining_pct: number | null; five_hour_resets_at: string | null;
  weekly_remaining_pct: number | null; weekly_resets_at: string | null; status: HeadroomStatus; source: string; observed_at: string }
export interface Choice { id: string; harness: Harness; model: string; effort?: string; roles: string[]; description: string; enabled: boolean }
export interface UsageTotals { input_tokens: number; output_tokens: number; cost: number }
export interface RunView { run_id: string; state: RunState; harness?: Harness; account?: string; model?: string;
  recent_events: Event[]; result?: unknown; usage: UsageTotals; worktree?: string; branch?: string;
  join_command?: string; join_cwd?: string; join_env?: string[]; name?: string; depth: number; parent_run?: string;
  error_kind?: ErrorKind; cwd?: string; created_at?: string; finished_at?: string }
export interface AccountView { name: string; harness: Harness; signed_in: SignInState; enabled: boolean; cap: number;
  active_runs: number; headroom: Headroom[]; error?: string; priority?: number; pools?: string[]; flags?: AccountFlag[] }
export interface AccountFlag { pool?: string; until: string; reason?: string }
export interface HarnessesView { accounts: AccountView[]; choices: Choice[] }
export interface RouteAlternative { choice_id: string; account?: string; score?: number }
export interface RouteResult { choice_id: string; harness: Harness; account: string; model: string; effort?: string;
  source: RouteSource; rationale: string; confidence?: number; probabilities: Record<string, number>;
  usage: UsageTotals; alternatives: RouteAlternative[]; config_fingerprint: string }
export interface RouteParams { prompt: string; role?: string; exclude?: string[]; harness?: string; account?: string; model?: string; effort?: string }
export interface MessageView { id: string; from_run?: string; to_run?: string; kind: MessageKind; body: string; created_at: string }
export interface SendMessageParams { text: string; kind?: MessageKind }
export interface StatusResult { pid: number; version: string; uptime_s: number; sessions: number; active_runs: number }
export interface AckResult { ok: boolean }
export interface AgentView { name: string; description: string }
export interface AgentsView { agents: AgentView[] }
export interface JoinView { argv: string[]; cwd: string; env_names: string[] }
export interface Page<T> { items: T[]; next_cursor: string | null }
export interface SessionSummary { id: string; kind: "orchestrator" | "interactive"; client: string; cwd: string; account?: string;
  started_at: string; ended_at?: string; status: "active" | "escalated" | "failed" | "done"; run_count: number; cost: number }
export interface SessionDetail extends SessionSummary { runs: RunView[] }
export interface DailyUsage { day: string; input_tokens: number; output_tokens: number; cost: number; by_account: Record<string, number> }
export interface DashboardView { status: StatusResult; caps: { global: number }; accounts: AccountView[]; live_runs: RunView[];
  escalations: Event[]; daily: DailyUsage[] }
export type LoginStart = { mode: "device_auth"; url: string; code: string; expires_at: string } | { mode: "manual"; command: string; cwd?: string };
export interface LoginStatus { signed_in: SignInState; mode: "device_auth" | "manual"; state: "pending" | "done" | "expired" | "failed"; message?: string }
export interface TranscriptLine { offset: number; role: "user" | "assistant" | "tool" | "system"; text: string; tool?: { name: string; ok?: boolean }; ts?: string }
export interface TranscriptPage { lines: TranscriptLine[]; next_offset: number | null; total_bytes: number }
export interface ApiErrorBody { error: { code: number; message: string; data?: unknown } }
export type SseMessage =
  | { event: "event"; data: Event } | { event: "run"; data: RunView } | { event: "session"; data: SessionSummary }
  | { event: "headroom"; data: Headroom } | { event: "account"; data: AccountView }
  | { event: "hello"; data: { seq: number; server_time: string } } | { event: "resync"; data: Record<string, never> };
```
(Workflow and config types are added by their tasks, T2.7 and T2.8, from the matching Rust types.)

## 7. Cross-cutting

### 7.1 Accessibility
- WCAG 2.2 AA. Every interactive element reachable by keyboard with the visible shadcn focus ring; skip-to-content link first in the shell; one `h1` per page.
- Colour is never the only signal: state badges and Gantt segments carry text, glyphs or patterns; headroom bars carry percent text.
- Dialogs, sheets and menus use Radix focus trapping and return focus. Toasts are `aria-live="polite"`; errors that block use `role="alert"`.
- Timeline: keyboard model and screen-reader summary table in 4.6. Tables use real `<table>` semantics with `scope` headers.
- Minimum touch target 40 px on `sm`.

### 7.2 Dark mode
Tailwind `dark` class strategy on `<html>`, three-way toggle (system default). All colours via CSS variables; the Gantt colours have their own light/dark pairs (4.3). `color-scheme` is set so scrollbars and form controls match. Charts read colours from `--chart-1..5`.

### 7.3 Responsive
- `lg` and up: sidebar expanded, tables full. `md`: sidebar collapsed to icons. `sm`: off-canvas sidebar; tables become horizontally scrollable inside `ScrollArea` with the first column sticky; dashboard stat cards stack 2 by 2; run panel `Sheet` full width.
- Timeline on `sm`: lane labels collapse to run initials plus the state dot with the full label in the tooltip; pinch zoom is not supported, the `Slider` is. Page works but is documented as best on desktop.

### 7.4 Empty, error, loading
- Loading: `Skeleton` shaped like the final content (table rows, cards, a grey lane stack for the timeline); no spinners except in buttons and login polling.
- Empty: sessions ("No sessions yet. Connect an orchestrator with `superorchestrator install`." with the command in `CommandBlock`), accounts ("Add your first account"), live runs ("No active runs"), choices, agents, workflows.
- Error: query errors render `ErrorAlert` (message, code, Retry). A global `QueryCache.onError` toasts background refetch errors once per minute. 401 redirects to `/login`; 503 `UNAVAILABLE` shows "Supervisor unavailable" with the header pill red. A React error boundary per route shows a fallback card, so a timeline crash does not take down navigation.
- Stream offline: amber pill and the 5 s polling fallback (2.3).

### 7.5 Security
- Loopback binding: no auth. Non-loopback: the server requires `--token`; unauthenticated API calls return 401; the SPA shows `TokenPage` (single `Input type=password`), posts `POST /auth`, and the server sets an HttpOnly, SameSite=Strict cookie. The token is never kept in JS, `localStorage` or URLs.
- Never displayed: API key values (OpenRouter, anything under credential homes), `JoinResult.env` values (API returns only names), auth file contents. The API never sends them, and the client types have no field for them. Join command display shows `argv` and `cwd` only.
- State-changing requests send `Content-Type: application/json` only (rejects simple cross-site form posts) and the server checks `Origin` equals the UI origin.
- Rendering untrusted text (transcripts, results, messages): React text nodes only, no `dangerouslySetInnerHTML`, no Markdown HTML. Links in transcripts are not auto-linked.
- CSP set by the server: `default-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'`; no third-party scripts or fonts at runtime (Geist is bundled via `@fontsource-variable/geist`).

## 8. Test plan

- Vitest + React Testing Library + jest-dom (already configured) + MSW (`msw/node` in `test/setup.ts`; same handlers back `pnpm dev:mock` through the service worker) + `@testing-library/user-event`.
- Unit (pure):
  - `deriveTimeline`: single run lifecycle; state sequence with waiting and stalled; failover across two attempts; tool call pairing (including unmatched finish); message arrows; escalated marker; open segment ends at `now`; terminal run closes; out-of-order `ts` handled; nested runs lane order; fold equivalence with `applyEvent`; 10 000-event benchmark under 200 ms.
  - `invalidationsFor`, `viewport` zoom/pan math, `nice` axis step, tick clustering, `contrast` of all state colour pairs in light and dark.
- Component:
  - `RunPanel`: tab switching, Route tab bars sorted and highlighted, Join tab copies argv (clipboard mocked) and never renders env values, action buttons enabled per state (`pending_review` shows accept/reject, terminal shows "Send and resume"), reject requires feedback.
  - `AccountsTable`: rows, headroom bars, enable switch calls `PATCH`, remove requires typing the name, login dialog for device auth shows URL and code and stops polling on `done`, manual mode shows the command.
  - `GanttCanvas` renders lanes for a fixture, click lane calls `onSelect`, keyboard Enter opens, hidden summary table present.
  - `SupervisorPill`/`CapMeter` states, theme toggle persists, `TokenPage` posts and redirects.
- Integration: `useEventStream` with a fake `EventSource` verifies invalidations and the polling fallback.
- Playwright smoke (later, T2.8+ follow-up): `pnpm e2e` against `superorchestrator ui` with a seeded `state.db`: dashboard loads, open a session, click a lane, panel shows tabs, add account dialog shows login command, dark mode toggle, timeline pan with 10 000 seeded events stays above 30 fps.
- CI: `pnpm lint`, `pnpm typecheck`, `pnpm test --run`, `pnpm build`. Type drift check: a script compares `api/types.ts` keys against `schemars` output (T2.3).

## 9. Task breakdown

Order: T2.3 first (everything depends on it). T2.4 to T2.7 can run in parallel after T2.3 (they touch separate `features/` folders). T2.8 last. Each task is sized for one implementer. Server-side work (axum `/api/v1`, SSE) is a separate Rust milestone task and is not in this list; the frontend uses MSW until it lands.

### T2.3 App shell, API client, stream (sonnet)
- Goal: router, shell, header, theme, typed API client, MSW setup, SSE hook.
- Acceptance:
  - Add shadcn components: `sidebar`, `sheet`, `tooltip`, `dropdown-menu`, `command`, `sonner`, `skeleton`, `alert`, `progress`, `separator`, `scroll-area`, `tabs`, `table`, `input`, `select`, `switch`, `dialog`, `alert-dialog`, `form`, `textarea`, `checkbox`, `toggle-group`, `slider`, `popover`, `collapsible`.
  - Routes from 1.2 render placeholder pages (lazy), `NotFound`, `TokenPage`.
  - `api/types.ts` complete per 6.1; `client.ts`, `endpoints.ts`, `queries.ts` for the whole 2.1 list; `ApiError`; 401 handling.
  - `stream.ts` with `useEventStream` (reconnect, Last-Event-ID, polling fallback flag) and pure `invalidationsFor` with tests.
  - Header: supervisor pill, cap meter, theme toggle, command palette.
  - MSW handlers and fixtures for status, dashboard, sessions, accounts, one session with ~40 events; `pnpm dev:mock`.
  - Tests pass: `pnpm lint typecheck test build`.
- Out of scope: page content, Rust server work, Playwright.

### T2.4 Dashboard and Sessions pages (haiku or sonnet; sonnet if the chart needs judgment)
- Goal: sections 3.1 and 3.2 against MSW.
- Acceptance: stat cards, accounts table with headroom bars (colour thresholds and `unknown` state), live runs, escalations, cost/tokens chart (recharts via shadcn `chart`), sessions table with filters mirrored in the URL, load more, empty/error/loading states, SSE patching verified in a test; component tests for `HeadroomBar` and the filter.
- Out of scope: timeline, account mutations.

### T2.5 Session timeline Gantt (sonnet; the hard one)
- Goal: sections 3.3 and 4 except the run panel tabs beyond a stub.
- Acceptance:
  - `derive.ts`, `applyEvent.ts` pure with all unit tests in section 8, benchmark test under 200 ms for 10 000 events.
  - `colors.ts` with CSS variables, patterns, legend, contrast test.
  - `GanttCanvas` (SVG) with lanes, nesting, axis, zoom/pan (wheel, keys, slider), minimap, time and lane culling, tick clustering, attempt bars, message arrows, markers.
  - Live follow and append through the rAF buffer; keyboard and screen-reader summary table; tooltips.
  - Selecting a lane opens the `Sheet` shell with the route `/sessions/:sid/runs/:rid`.
  - A seeded fixture with 10 000 events renders and pans smoothly (manual check recorded in the PR).
- Out of scope: run panel tab contents (T2.6), canvas renderer.

### T2.6 Run panel (sonnet)
- Goal: section 4.9.
- Acceptance: all seven tabs against MSW, virtualised Events and Transcript lists (10 000 rows fixture), `ProbabilityBars`, Join tab with copy and no env values, actions (send message, cancel, accept, reject with feedback) with toasts and invalidation, state-dependent enablement, component tests per section 8.
- Out of scope: Gantt drawing, server endpoints.

### T2.7 Accounts and Choices/routing pages (sonnet)
- Goal: sections 3.5 and 5.
- Acceptance: accounts table and all dialogs/sheets (add, login with device-auth and manual polling flows, import scan, edit, flag, remove with typed confirmation), choices CRUD, instruction editor, route preview with `ProbabilityBars`, decision log pagination; form validation; tests for login polling (fake timers) and remove confirmation.
- Out of scope: Rust account commands, agents, workflows.

### T2.8 Agents, Workflows, Settings and polish (haiku for Agents/Workflows lists; sonnet for Settings and the pass)
- Goal: sections 3.6 to 3.8 and cross-cutting 7.
- Acceptance: pages built against MSW; Settings never renders a secret value (test asserts the key source display only); error boundaries per route; skip link; responsive checks at 375, 768, 1280 px; axe (`vitest-axe`) run on every page with zero serious violations; Playwright smoke scaffold with one spec.
- Out of scope: Rust server work, `ui --install-service`.

## 10. Open questions

1. `AccountView` lacks `priority`, `pools` and flags; the Accounts page needs them (6.1 adds them as optional fields). The API task must extend `AccountView` or add an `AccountDetail`.
2. Where the `route_decision` brief and the spawn options live for the Brief tab: assumed in the `run_queued` event data and `route_decision` event data; SPEC 12 does not fix their shape.
3. `GET /sessions/:id/events` cursor semantics and an upper bound (10 000 events in memory is assumed fine; the server should reject unbounded pulls with `limit` max 1000).
4. Whether the supervisor can start `codex login --device-auth` and capture URL and code over the API (assumed yes, with a login task id held server side for 10 minutes).
5. Daily usage aggregate source: assumed computed from the `ledger` table on request.
6. Browser clipboard needs a secure context; loopback `http://127.0.0.1` qualifies, a non-loopback plain-HTTP bind does not (fallback: select-all text, no failure toast).
