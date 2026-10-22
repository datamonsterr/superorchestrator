/**
 * API types for `/api/v1` (docs/ui/design.md 2.1 and 6.1).
 *
 * Hand-written stubs, one-to-one with `crates/so-core/src/proto.rs`. T2.1 will
 * generate the same types from schemars; this file then re-exports them. Page
 * tasks must not edit this file.
 */

export type Harness = 'codex' | 'agy' | 'opencode' | 'claude'
export type RunState =
  | 'queued'
  | 'starting'
  | 'running'
  | 'waiting'
  | 'stalled'
  | 'escalated'
  | 'pending_review'
  | 'succeeded'
  | 'failed'
  | 'cancelled'
export type ErrorKind = 'quota' | 'rate_limit' | 'auth' | 'other'
export type Isolation = 'worktree' | 'none'
export type MessageKind = 'message' | 'progress' | 'question' | 'answer' | 'nudge' | 'result'
export type SignInState = 'signed_in' | 'signed_out' | 'unknown'
export type HeadroomStatus = 'ok' | 'low' | 'exhausted' | 'unknown'
export type RouteSource = 'jev' | 'heuristic' | 'pinned'
export type EventType =
  | 'run_queued'
  | 'run_started'
  | 'state_changed'
  | 'run_finished'
  | 'attempt_started'
  | 'failover'
  | 'route_decision'
  | 'tool_call_started'
  | 'tool_call_finished'
  | 'assistant_message'
  | 'progress'
  | 'message_sent'
  | 'question_asked'
  | 'question_answered'
  | 'usage'
  | 'user_joined'
  | 'user_left'
  | 'join_command_issued'
  | 'stall_detected'
  | 'nudge_sent'
  | 'escalated'
  | 'worktree_created'
  | 'worktree_removed'
  | 'check_result'
  | 'review_accepted'
  | 'review_rejected'

export interface Event {
  id: string
  session_id: string
  run_id: string | null
  ts: string
  type: EventType
  data: unknown
}
export interface Headroom {
  account: string
  pool: string
  five_hour_remaining_pct: number | null
  five_hour_resets_at: string | null
  weekly_remaining_pct: number | null
  weekly_resets_at: string | null
  status: HeadroomStatus
  source: string
  observed_at: string
}
export interface Choice {
  id: string
  harness: Harness
  model: string
  effort?: string
  roles: string[]
  description: string
  enabled: boolean
}
export interface UsageTotals {
  input_tokens: number
  output_tokens: number
  cost: number
}
/** `RunView` minus `join_command`, `join_cwd`, `join_env`. */
export interface RunViewPublic {
  run_id: string
  state: RunState
  harness?: Harness
  account?: string
  model?: string
  recent_events: Event[]
  result?: unknown
  usage: UsageTotals
  worktree?: string
  branch?: string
  name?: string
  depth: number
  parent_run?: string
  error_kind?: ErrorKind
  cwd?: string
  created_at?: string
  finished_at?: string
}
export interface AccountFlag {
  pool?: string
  until: string
  reason?: string
}
export interface AccountView {
  name: string
  harness: Harness
  signed_in: SignInState
  enabled: boolean
  cap: number
  active_runs: number
  headroom: Headroom[]
  error?: string
  priority?: number
  pools?: string[]
  flags?: AccountFlag[]
}
export interface HarnessesView {
  accounts: AccountView[]
  choices: Choice[]
}
export interface RouteAlternative {
  choice_id: string
  account?: string
  score?: number
}
export interface RouteResult {
  choice_id: string
  harness: Harness
  account: string
  model: string
  effort?: string
  source: RouteSource
  rationale: string
  confidence?: number
  probabilities: Record<string, number>
  usage: UsageTotals
  alternatives: RouteAlternative[]
  config_fingerprint: string
}
export interface RouteParams {
  prompt: string
  role?: string
  exclude?: string[]
  harness?: string
  account?: string
  model?: string
  effort?: string
}
export interface RouteRecord {
  ts: string
  run_id?: string
  prompt_excerpt: string
  result: RouteResult
}
export interface MessageView {
  id: string
  from_run?: string
  to_run?: string
  kind: MessageKind
  body: string
  created_at: string
}
export interface SendMessageParams {
  text: string
  kind?: MessageKind
}
export interface StatusResult {
  pid: number
  version: string
  uptime_s: number
  sessions: number
  active_runs: number
}
/** `GET /status`: StatusResult plus `caps`. */
export interface StatusView extends StatusResult {
  caps: { global: number }
}
export interface AckResult {
  ok: boolean
}
export interface AgentView {
  name: string
  description: string
}
export interface AgentsView {
  agents: AgentView[]
}
export interface AgentDetail {
  name: string
  description: string
  path: string
  source: string
}
/** Response of `POST /agents` and `PUT /agents/:name`. */
export interface AgentWriteResult {
  ok: boolean
  warnings: string[]
}
export interface JoinView {
  argv: string[]
  cwd: string
  env_names: string[]
}
export interface Page<T> {
  items: T[]
  next_cursor: string | null
}
export type SessionStatus = 'active' | 'escalated' | 'failed' | 'done'
export interface SessionSummary {
  id: string
  kind: 'orchestrator' | 'interactive'
  client: string
  cwd: string
  account?: string
  started_at: string
  ended_at?: string
  status: SessionStatus
  run_count: number
  cost: number
}
export interface SessionDetail extends SessionSummary {
  runs: RunViewPublic[]
}
export interface DailyUsage {
  day: string
  input_tokens: number
  output_tokens: number
  cost: number
  by_account: Record<string, number>
}
export interface DashboardView {
  status: StatusResult
  caps: { global: number }
  accounts: AccountView[]
  live_runs: RunViewPublic[]
  escalations: Event[]
  daily: DailyUsage[]
}
export type SignInStart =
  | { mode: 'device_auth'; url: string; code: string; expires_at: string }
  | { mode: 'manual'; command: string; cwd?: string }
export interface SignInStatus {
  signed_in: SignInState
  mode: 'device_auth' | 'manual'
  state: 'pending' | 'done' | 'expired' | 'failed'
  message?: string
}
export interface ImportCandidate {
  harness: Harness
  name: string
  home: string
}
export interface TranscriptLine {
  offset: number
  role: 'user' | 'assistant' | 'tool' | 'system'
  text: string
  tool?: { name: string; ok?: boolean }
  ts?: string
}
export interface TranscriptPage {
  lines: TranscriptLine[]
  next_offset: number | null
  total_bytes: number
}
export interface ApiErrorBody {
  error: { code: number; message: string; data?: unknown }
}

// ---- Choices, models, worktrees, doctor (stubs matching 2.1) ----

export interface ChoicesView {
  choices: Choice[]
  routing_instruction: string
}
export interface ModelEntry {
  harness: Harness
  id: string
  efforts: string[]
}
export interface ModelCatalog {
  refreshed_at: string | null
  models: ModelEntry[]
}
export interface WorktreeView {
  path: string
  branch: string
  run_id?: string
  size_bytes: number
  orphaned: boolean
}
export interface PruneResult {
  removed: string[]
}
export interface DoctorCheck {
  name: string
  ok: boolean
  detail: string
}
export interface DoctorReport {
  checks: DoctorCheck[]
}

// ---- Workflows (stubs; T2.8 refines from the Rust types) ----

export type WorkflowState = 'queued' | 'running' | 'succeeded' | 'failed' | 'cancelled'
export interface WorkflowSummary {
  name: string
  description?: string
  path?: string
}
export interface WorkflowStep {
  name: string
  state: WorkflowState
  run_id?: string
  started_at?: string
  finished_at?: string
}
export interface WorkflowRunView {
  id: string
  name: string
  state: WorkflowState
  started_at: string
  finished_at?: string
  session_id?: string
}
export interface WorkflowStatusResult extends WorkflowRunView {
  result?: unknown
  error?: string
}
export interface WorkflowRunDetail extends WorkflowStatusResult {
  steps: WorkflowStep[]
}
export interface RunWorkflowResult {
  id: string
  session_id?: string
}

// ---- Config (stub; T2.8 refines) ----

/** Secrets are never sent; only where they come from. */
export interface SecretSource {
  source: 'env' | 'file' | 'unset'
  name?: string
}
export interface ConfigView {
  platform?: 'linux' | 'macos' | 'windows'
  caps: { global: number; per_account?: number }
  stall_ms: number
  notifications: { enabled: boolean; command?: string }
  worktree: { root?: string; default_isolation?: Isolation }
  routing: { min_confidence: number; min_margin: number }
  openrouter: { key: SecretSource }
  ui: { bind: string; port: number }
}
export type ConfigPatch = {
  [K in keyof ConfigView]?: ConfigView[K] extends object ? Partial<ConfigView[K]> : ConfigView[K]
}

// ---- Server-sent events ----

export interface WorkflowEventData {
  id: string
  name: string
  state: string
  step?: WorkflowStep
  session_id?: string
  ts: string
}
export type SseMessage =
  | { event: 'event'; data: Event }
  | { event: 'run'; data: Omit<RunViewPublic, 'recent_events'> }
  | { event: 'session'; data: SessionSummary }
  | { event: 'headroom'; data: Headroom }
  | { event: 'account'; data: AccountView }
  | { event: 'workflow'; data: WorkflowEventData }
  | { event: 'hello'; data: { seq: number; server_time: string; boot_id: string } }
  | { event: 'resync'; data: Record<string, never> }
