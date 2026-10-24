import type {
  AccountView,
  AgentDetail,
  AgentView,
  ChoicesView,
  ConfigView,
  DailyUsage,
  DashboardView,
  DoctorReport,
  Event,
  EventType,
  Headroom,
  ImportCandidate,
  MessageView,
  ModelCatalog,
  RouteRecord,
  RouteResult,
  RunViewPublic,
  SessionDetail,
  SessionSummary,
  StatusView,
  TranscriptLine,
  WorkflowRunDetail,
  WorkflowRunView,
  WorkflowSummary,
  WorktreeView,
} from '../types'

const T0 = Date.UTC(2026, 0, 1, 12, 0, 0)
const at = (min: number) => new Date(T0 + min * 60_000).toISOString()

export const headroom = (account: string, pool: string, five: number | null, weekly: number | null): Headroom => ({
  account,
  pool,
  five_hour_remaining_pct: five,
  five_hour_resets_at: five === null ? null : at(180),
  weekly_remaining_pct: weekly,
  weekly_resets_at: weekly === null ? null : at(60 * 72),
  status: five === null ? 'unknown' : five < 10 ? 'exhausted' : five < 30 ? 'low' : 'ok',
  source: five === null ? 'ledger' : 'probe',
  // Never observed is null on the wire, not the epoch.
  observed_at: five === null ? null : at(0),
})

export const accounts: AccountView[] = [
  {
    name: 'codex-acc1',
    harness: 'codex',
    signed_in: 'signed_in',
    enabled: true,
    cap: 2,
    active_runs: 1,
    priority: 1,
    pools: ['main'],
    headroom: [headroom('codex-acc1', 'main', 72, 61)],
  },
  {
    name: 'agy-acc1',
    harness: 'agy',
    signed_in: 'signed_out',
    enabled: true,
    cap: 1,
    active_runs: 0,
    priority: 2,
    pools: ['gemini', 'claude'],
    headroom: [headroom('agy-acc1', 'gemini', 40, 22), headroom('agy-acc1', 'claude', null, null)],
    flags: [{ pool: 'claude', until: at(300), reason: 'quota' }],
  },
  {
    name: 'claude-acc1',
    harness: 'claude',
    signed_in: 'signed_in',
    enabled: false,
    cap: 1,
    active_runs: 0,
    priority: 3,
    pools: ['main'],
    headroom: [headroom('claude-acc1', 'main', 5, 80)],
  },
]

export const choices: ChoicesView = {
  choices: [
    { id: 'codex-high', harness: 'codex', model: 'gpt-5', effort: 'high', roles: ['code', 'review'], description: 'Hard coding tasks', enabled: true },
    { id: 'agy-pro', harness: 'agy', model: 'gemini-pro', roles: ['code'], description: 'Long context', enabled: true },
  ],
  routing_instruction: 'Prefer codex-high for refactors; use agy-pro for large-context reading.',
  restart_required: false,
}

export const evt = (id: string, type: EventType, run_id: string | null, min: number, data: unknown = {}, session = 'sess-1'): Event => ({
  id,
  session_id: session,
  run_id,
  ts: at(min),
  type,
  data,
})

export const runs: RunViewPublic[] = [
  {
    run_id: 'run-1',
    session_id: 'sess-1',
    state: 'running',
    harness: 'codex',
    account: 'codex-acc1',
    model: 'gpt-5',
    recent_events: [],
    usage: { input_tokens: 12000, output_tokens: 3400, cost: 0.42 },
    worktree: '/work/wt/run-1',
    branch: 'so/run-1',
    name: 'refactor-api',
    depth: 0,
    created_at: at(0),
    cwd: '/work/repo',
  },
  {
    run_id: 'run-2',
    session_id: 'sess-1',
    state: 'pending_review',
    harness: 'agy',
    account: 'agy-acc1',
    model: 'gemini-pro',
    recent_events: [],
    usage: { input_tokens: 5000, output_tokens: 900, cost: 0.12 },
    name: 'write-tests',
    depth: 1,
    parent_run: 'run-1',
    created_at: at(5),
    cwd: '/work/repo',
  },
]

export const events: Event[] = [
  evt('ev-0001', 'run_queued', 'run-1', 0, { brief: 'Refactor the API' }),
  evt('ev-0002', 'run_started', 'run-1', 1),
  evt('ev-0003', 'state_changed', 'run-1', 1, { from: 'starting', to: 'running' }),
  evt('ev-0004', 'run_queued', 'run-2', 5, { brief: 'Write tests' }),
  evt('ev-0005', 'stall_detected', 'run-2', 9, { idle_ms: 60000 }),
  evt('ev-0006', 'escalated', 'run-2', 12, { reason: 'stalled after 3 nudges' }),
]

export const sessions: SessionSummary[] = [
  { id: 'sess-1', kind: 'orchestrator', client: 'claude-code', cwd: '/work/repo', started_at: at(0), status: 'active', run_count: 2, cost: 0.54 },
  { id: 'sess-2', kind: 'interactive', client: 'codex-acc1', cwd: '/work/other', account: 'codex-acc1', started_at: at(-90), ended_at: at(-60), status: 'done', run_count: 0, cost: 0.1 },
  { id: 'sess-3', kind: 'orchestrator', client: 'claude-code', cwd: '/work/repo', started_at: at(-300), ended_at: at(-280), status: 'failed', run_count: 1, cost: 1.2 },
]

export const sessionDetail: SessionDetail = { ...sessions[0], runs }

export const messages: MessageView[] = [
  { id: 'msg-1', from_run: 'run-1', to_run: 'run-2', kind: 'message', body: 'Please add tests for the new handler.', created_at: at(5) },
  { id: 'msg-2', from_run: 'run-2', to_run: 'run-1', kind: 'question', body: 'Which fixture should I use?', created_at: at(8) },
]

export const routeResult: RouteResult = {
  choice_id: 'codex-high',
  harness: 'codex',
  account: 'codex-acc1',
  model: 'gpt-5',
  effort: 'high',
  source: 'jev',
  rationale: 'Refactoring benefits from the strongest coding model.',
  confidence: 0.82,
  complexity: 82.5,
  level: 'high',
  probabilities: { '0': 0, '1': 0.03, '2': 0.15, '3': 0.62, '4': 0.2 },
  usage: { input_tokens: 800, output_tokens: 40, cost: 0.0004 },
  alternatives: [{ choice_id: 'agy-pro', account: 'agy-acc1', score: 0.21 }],
  config_fingerprint: 'fp-1',
}

export const routeRecords: RouteRecord[] = [
  { ts: at(0), run_id: 'run-1', prompt_excerpt: 'Refactor the API...', result: routeResult },
  { ts: at(5), run_id: 'run-2', prompt_excerpt: 'Write tests...', result: { ...routeResult, choice_id: 'agy-pro', harness: 'agy', account: 'agy-acc1', model: 'gemini-pro', source: 'heuristic' } },
]

export const transcript: TranscriptLine[] = [
  { offset: 0, role: 'user', text: 'Refactor the API handlers.' },
  { offset: 27, role: 'assistant', text: 'Reading the handlers first.' },
  { offset: 55, role: 'tool', text: 'read_file src/api.rs', tool: { name: 'read_file', ok: true } },
]

export const daily: DailyUsage[] = Array.from({ length: 30 }, (_, i) => ({
  day: new Date(T0 - (29 - i) * 86_400_000).toISOString().slice(0, 10),
  input_tokens: 100_000 + i * 5000,
  output_tokens: 20_000 + i * 1000,
  cost: 1 + (i % 7) * 0.5,
  by_account: { 'codex-acc1': 0.6 + (i % 5) * 0.2, 'agy-acc1': 0.4 + (i % 3) * 0.1 },
}))

export const status: StatusView = { pid: 4242, version: '0.1.0', uptime_s: 10_800, sessions: 3, active_runs: 3, caps: { global: 8 } }

export const dashboard: DashboardView = {
  status,
  accounts,
  live_runs: runs,
  escalations: [events[5]],
  daily,
}

export const doctor: DoctorReport = {
  ok: false,
  checks: [
    { name: 'supervisor socket', ok: true, status: 'ok', detail: 'reachable' },
    { name: 'codex binary', ok: true, status: 'ok', detail: '/usr/bin/codex' },
    { name: 'agy binary', ok: false, status: 'fail', detail: 'not found on PATH' },
  ],
}

export const models: ModelCatalog = {
  refreshed_at: at(-10),
  models: [
    { harness: 'codex', id: 'gpt-5', efforts: ['low', 'medium', 'high'], cost: '$$', tier: 'high' },
    { harness: 'agy', id: 'gemini-pro', efforts: [], cost: '$', tier: 'mid' },
  ],
}

export const worktrees: WorktreeView[] = [
  { path: '/work/wt/run-1', branch: 'so/run-1', run_id: 'run-1', size_bytes: 12_345_678, orphaned: false },
  { path: '/work/wt/old', branch: 'so/old', run_id: 'run-0', size_bytes: 2_000_000, orphaned: true },
]

export const scanCandidates: ImportCandidate[] = [{ harness: 'codex', name: 'codex-acc9', home: '/home/me/.codex-acc9' }]

const agentSource = (name: string, description: string, body = 'Review carefully.') =>
  `---\nname: ${name}\ndescription: ${description}\n---\n${body}\n`

export const agents: AgentDetail[] = [
  {
    name: 'reviewer',
    description: 'Reviews diffs for bugs',
    path: '/home/me/.config/superorchestrator/agents/reviewer/AGENT.md',
    origin: 'user',
    read_only: false,
    redacted: false,
    source: agentSource('reviewer', 'Reviews diffs for bugs'),
  },
  {
    name: 'imported',
    description: 'Imported from .claude/agents',
    path: '/work/.claude/agents/imported.md',
    origin: 'claude',
    read_only: true,
    redacted: false,
    source: agentSource('imported', 'Imported from .claude/agents'),
  },
  {
    name: 'secretive',
    description: 'Has a secret in its file',
    path: '/home/me/.config/superorchestrator/agents/secretive/AGENT.md',
    origin: 'user',
    read_only: false,
    redacted: true,
    source: agentSource('secretive', 'Has a secret in its file', 'token: [REDACTED]'),
  },
]

export const agentViews: AgentView[] = [
  { name: 'reviewer', description: 'Reviews diffs for bugs', source: 'user', path: agents[0].path, harness: 'auto' },
  { name: 'imported', description: 'Imported from .claude/agents', source: 'claude', path: agents[1].path, harness: 'claude' },
  { name: 'secretive', description: 'Has a secret in its file', source: 'user', path: agents[2].path, harness: 'codex' },
]

export const workflows: WorkflowSummary[] = [
  { name: 'ship-feature', path: '/work/.superorchestrator/workflows/ship-feature.js', source: 'project' },
]

export const workflowRuns: WorkflowRunView[] = [
  { id: 'wf-1', name: 'ship-feature', state: 'running', started_at: at(0), session_id: 'sess-1' },
  { id: 'wf-0', name: 'ship-feature', state: 'succeeded', started_at: at(-120), finished_at: at(-90) },
]

export const workflowRunDetail: WorkflowRunDetail = {
  ...workflowRuns[0],
  steps: [
    { index: 0, name: 'plan', state: 'succeeded', run_id: 'run-1', started_at: at(0), finished_at: at(3) },
    { index: 1, name: 'implement', state: 'running', run_id: 'run-2', started_at: at(3) },
  ],
}

export const config: ConfigView = {
  platform: 'linux',
  restart_required: false,
  ui_token: { source: 'unset', name: null },
  openrouter_key: { source: 'env', name: 'OPENROUTER_API_KEY' },
  config: {
    limits: { global_cap: 8, max_children: 5 },
    runs: { stall_ms: 300_000, ask_timeout: '10m' },
    notifications: { desktop: false, discord_webhook_env: '', on: ['escalated'] },
    worktrees: { dir: '', branch_prefix: 'so/' },
    router: {
      policy: { min_confidence: 0.5, min_margin: 0.1 },
      levels: [
        { name: 'low', min: 0, max: 39, tier: 'fast' },
        { name: 'high', min: 40, max: 100, choices: [{ id: 'codex-high', priority: 1 }, { id: 'agy-pro' }] },
      ],
    },
    ui: { bind: '127.0.0.1', port: 7878, enabled: false },
    defaults: { harness: null, model: null, effort: null },
  },
}
