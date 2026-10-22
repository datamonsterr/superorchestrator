import { request } from './client'
import type {
  AckResult,
  AccountView,
  AgentDetail,
  AgentWriteResult,
  AgentsView,
  ChoicesView,
  ConfigPatch,
  ConfigView,
  DailyUsage,
  DashboardView,
  DoctorReport,
  Event,
  HarnessesView,
  ImportCandidate,
  JoinView,
  MessageView,
  ModelCatalog,
  Page,
  PruneRequest,
  PruneResult,
  RouteParams,
  RouteRecord,
  RouteResult,
  RunViewPublic,
  RunWorkflowResult,
  SendMessageParams,
  SessionDetail,
  SessionStatus,
  SessionSummary,
  SignInStart,
  SignInStatus,
  StatusView,
  TranscriptPage,
  WorkflowRunDetail,
  WorkflowRunView,
  WorkflowSummary,
  WorktreeView,
  Harness,
  EventType,
} from './types'

type Opt = { signal?: AbortSignal }
const enc = encodeURIComponent

/** One typed function per endpoint in design 2.1. */
export const endpoints = {
  // status and overview
  getStatus: (o?: Opt) => request<StatusView>('/status', o),
  getDashboard: (o?: Opt) => request<DashboardView>('/dashboard', o),
  getDailyUsage: (days = 30, o?: Opt) => request<DailyUsage[]>('/usage/daily', { query: { days }, ...o }),
  getDoctor: (o?: Opt) => request<DoctorReport>('/doctor', o),
  getHarnesses: (o?: Opt) => request<HarnessesView>('/harnesses', o),

  // sessions and runs
  listSessions: (q: { status?: SessionStatus; q?: string; limit?: number; cursor?: string | null } = {}, o?: Opt) =>
    request<Page<SessionSummary>>('/sessions', { query: q, ...o }),
  getSession: (id: string, o?: Opt) => request<SessionDetail>(`/sessions/${enc(id)}`, o),
  listSessionEvents: (id: string, q: { after?: string | null; limit?: number } = {}, o?: Opt) =>
    request<Page<Event>>(`/sessions/${enc(id)}/events`, { query: q, ...o }),
  getRun: (id: string, o?: Opt) => request<RunViewPublic>(`/runs/${enc(id)}`, o),
  listRunEvents: (
    id: string,
    q: { after?: string | null; type?: EventType; limit?: number } = {},
    o?: Opt,
  ) => request<Page<Event>>(`/runs/${enc(id)}/events`, { query: q, ...o }),
  getTranscript: (id: string, q: { offset?: number; limit?: number } = {}, o?: Opt) =>
    request<TranscriptPage>(`/runs/${enc(id)}/transcript`, { query: q, ...o }),
  getRunRoute: (id: string, o?: Opt) => request<RouteResult>(`/runs/${enc(id)}/route`, o),
  listRunMessages: (id: string, o?: Opt) => request<MessageView[]>(`/runs/${enc(id)}/messages`, o),
  sendMessage: (id: string, body: SendMessageParams) =>
    request<AckResult>(`/runs/${enc(id)}/messages`, { method: 'POST', body }),
  cancelRun: (id: string) => request<AckResult>(`/runs/${enc(id)}/cancel`, { method: 'POST' }),
  acceptRun: (id: string) => request<AckResult>(`/runs/${enc(id)}/accept`, { method: 'POST' }),
  rejectRun: (id: string, feedback: string) =>
    request<AckResult>(`/runs/${enc(id)}/reject`, { method: 'POST', body: { feedback } }),
  joinRun: (id: string) => request<JoinView>(`/runs/${enc(id)}/join`, { method: 'POST' }),

  // routing
  route: (body: RouteParams) => request<RouteResult>('/route', { method: 'POST', body }),
  listRouteRecords: (q: { limit?: number; cursor?: string | null } = {}, o?: Opt) =>
    request<Page<RouteRecord>>('/routing/records', { query: q, ...o }),
  getChoices: (o?: Opt) => request<ChoicesView>('/choices', o),
  putChoices: (body: ChoicesView) => request<ChoicesView>('/choices', { method: 'PUT', body }),
  getModels: (o?: Opt) => request<ModelCatalog>('/models', o),
  refreshModels: () => request<ModelCatalog>('/models/refresh', { method: 'POST' }),

  // accounts
  listAccounts: (o?: Opt) => request<AccountView[]>('/accounts', o),
  createAccount: (body: { harness: Harness; name: string; home?: string }) =>
    request<AccountView>('/accounts', { method: 'POST', body }),
  importAccount: (body: { name: string; harness: Harness; home: string }) =>
    request<AccountView>('/accounts/import', { method: 'POST', body }),
  scanAccounts: (o?: Opt) => request<ImportCandidate[]>('/accounts/scan', o),
  patchAccount: (
    name: string,
    body: { cap?: number; priority?: number; enabled?: boolean; pools?: string[] },
  ) => request<AccountView>(`/accounts/${enc(name)}`, { method: 'PATCH', body }),
  removeAccount: (name: string, purge = false) =>
    request<AckResult>(`/accounts/${enc(name)}`, { method: 'DELETE', query: { purge: purge || undefined } }),
  flagAccount: (
    name: string,
    body: { pool?: string; until?: string; hours?: number; clear?: boolean; reason?: string },
  ) => request<AccountView>(`/accounts/${enc(name)}/flag`, { method: 'POST', body }),
  startLogin: (name: string) => request<SignInStart>(`/accounts/${enc(name)}/login`, { method: 'POST' }),
  getLoginStatus: (name: string, o?: Opt) => request<SignInStatus>(`/accounts/${enc(name)}/login`, o),

  // worktrees
  listWorktrees: (o?: Opt) => request<WorktreeView[]>('/worktrees', o),
  pruneWorktrees: (body: PruneRequest = {}) =>
    request<PruneResult>('/worktrees/prune', { method: 'POST', body }),

  // agents
  listAgents: (o?: Opt) => request<AgentsView>('/agents', o),
  createAgent: (body: { name: string; source: string }) =>
    request<AgentWriteResult>('/agents', { method: 'POST', body }),
  getAgent: (name: string, o?: Opt) => request<AgentDetail>(`/agents/${enc(name)}`, o),
  putAgent: (name: string, body: { source: string }) =>
    request<AgentWriteResult>(`/agents/${enc(name)}`, { method: 'PUT', body }),
  deleteAgent: (name: string) => request<AckResult>(`/agents/${enc(name)}`, { method: 'DELETE' }),

  // workflows
  listWorkflows: (o?: Opt) => request<WorkflowSummary[]>('/workflows', o),
  listWorkflowRuns: (q: { limit?: number; cursor?: string | null } = {}, o?: Opt) =>
    request<Page<WorkflowRunView>>('/workflows/runs', { query: q, ...o }),
  getWorkflowRun: (id: string, o?: Opt) => request<WorkflowRunDetail>(`/workflows/runs/${enc(id)}`, o),
  runWorkflow: (name: string, args?: unknown) =>
    request<RunWorkflowResult>(`/workflows/${enc(name)}/run`, { method: 'POST', body: { args } }),

  // config and auth
  getConfig: (o?: Opt) => request<ConfigView>('/config', o),
  patchConfig: (body: ConfigPatch) => request<ConfigView>('/config', { method: 'PATCH', body }),
  postAuth: (token: string) => request<void>('/auth', { method: 'POST', body: { token } }),
} as const
