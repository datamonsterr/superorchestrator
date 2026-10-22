/**
 * API types for `/api/v1` (docs/ui/design.md 2.1 and 6.1).
 *
 * Every wire shape comes from `generated.ts` (`cargo run -p so-web --bin gen-ts`), so the Rust
 * types are the single source of truth and `tsc` checks the MSW fixtures against them.
 *
 * The generator marks every `#[serde(default)]` field optional, because it describes what the
 * server accepts. Responses always carry those fields, so `Always` makes them required again for
 * the response types the pages read. Only types that exist in the web app alone are written here.
 */

import type {
  Check,
  Config,
  RouteUsage,
  SessionDetail as GenSessionDetail,
  UsageTotals as GenUsage,
  Event,
  Headroom,
  SessionSummary,
  SendMessageBody,
  WorkflowEventData,
  WorkflowStatusResult as GenWorkflowStatus,
  ModelRow,
  AccountView as GenAccountView,
  RunViewPublic as GenRun,
  Choice as GenChoice,
  ChoicesView as GenChoices,
  RouteResult as GenRoute,
  DashboardView as GenDashboard,
} from './generated'

export type { Page, SessionSummary } from './generated'
export type { Event, Headroom }

/** `T` with the listed fields required: the server always serialises them. */
type Always<T, K extends keyof T> = Omit<T, K> & Required<Pick<T, K>>

export type AccountView = Always<GenAccountView, 'headroom'>
export type UsageTotals = Required<GenUsage>
export type RunViewPublic = Omit<Always<GenRun, 'recent_events' | 'depth'>, 'usage'> & { usage: UsageTotals }
export type SessionDetail = Omit<GenSessionDetail, 'runs'> & { runs: RunViewPublic[] }
export type Choice = Always<GenChoice, 'roles' | 'description' | 'enabled'>
export type ChoicesView = Omit<GenChoices, 'choices'> & { choices: Choice[] }
export type RouteResult = Omit<Always<GenRoute, 'alternatives' | 'probabilities' | 'config_fingerprint'>, 'usage'> & { usage: Required<RouteUsage> }
/** Workflow and step states are open strings on the wire (`running`, `waiting`, `stalled`, `cached`, ...). */
export type WorkflowState = string
/** Body of `PUT /choices`: the web layer only needs these two fields. */
export type ChoicesPut = { choices: Choice[]; routing_instruction: string }
export type SendMessageParams = SendMessageBody
export type HarnessesView = { accounts: AccountView[]; choices: Choice[] }
export type DashboardView = Omit<GenDashboard, 'accounts' | 'live_runs'> & { accounts: AccountView[]; live_runs: RunViewPublic[] }
export type WorkflowRunDetail = Always<GenWorkflowStatus, 'steps'>

export type {
  AccountFlag,
  AccountsCfg,
  AckResult,
  AddAccountRequest,
  AgentDetail,
  AgentView,
  AgentWriteResult,
  AgentsView,
  ApiErrorBody,
  ApiErrorCode,
  ApiErrorDetail,
  Caps,
  ConfigView,
  CreateAgentRequest,
  DailyUsage,
  Defaults,
  DoctorReport,
  ErrorKind,
  EventType,
  FlagRequest,
  ForkBody,
  Harness,
  HeadroomSource,
  HeadroomStatus,
  HumanDuration,
  ImportAccountRequest,
  ImportCandidate,
  Jev,
  JoinView,
  Limits,
  MessageKind,
  MessageView,
  Notifications,
  PatchAccountRequest,
  Policy,
  PruneRequest,
  PruneResult,
  RejectBody,
  RouteAlternative,
  RouteParams,
  RouteRecord,
  RouteSource,
  RouteUsage,
  Router,
  RunState,
  RunWorkflowBody,
  RunWorkflowResult,
  Runs,
  SecretOrigin,
  SecretSource,
  SendMessageBody,
  SessionKind,
  SessionStatus,
  SignInMode,
  SignInProgress,
  SignInStart,
  SignInState,
  SignInStatus,
  SpawnResult,
  Status,
  StatusResult,
  StatusView,
  StreamHello,
  Supervisor,
  TokenRequest,
  TranscriptLine,
  TranscriptPage,
  TranscriptRole,
  TranscriptTool,
  Ui,
  UpdateAgentRequest,
  Usage,
  Weights,
  WorkflowRunView,
  WorkflowStep,
  WorkflowSummary,
  Workflows,
  WorktreeView,
  Worktrees,
} from './generated'

export type { ModelCatalog } from './generated'
export type DoctorCheck = Check
export type ModelEntry = ModelRow
/** Body of `PATCH /config`: a partial config object (each leaf is a dotted key on the server). */
export type ConfigPatch = Config

// ---- Server-sent events ----

export type SseMessage =
  | { event: 'event'; data: Event }
  | { event: 'run'; data: Omit<RunViewPublic, 'recent_events'> }
  | { event: 'session'; data: SessionSummary }
  | { event: 'headroom'; data: Headroom }
  | { event: 'account'; data: AccountView }
  | { event: 'workflow'; data: WorkflowEventData }
  | { event: 'hello'; data: { seq: number; server_time: string; boot_id: string } }
  | { event: 'resync'; data: Record<string, never> }
