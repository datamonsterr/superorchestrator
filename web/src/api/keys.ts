import type { EventType, SessionStatus } from './types'

/**
 * Query key factory (design 2.2). Hooks and `invalidationsFor` share these.
 * Detail and sub-resources have disjoint roots (`view` / `summary` / `events` /
 * `transcript` ...), so invalidating one never refetches another by prefix.
 */
export const qk = {
  status: () => ['status'] as const,
  dashboard: () => ['dashboard'] as const,
  usageDaily: (days: number) => ['usage', 'daily', days] as const,
  usageDailyAll: () => ['usage', 'daily'] as const,
  doctor: () => ['doctor'] as const,
  harnesses: () => ['harnesses'] as const,
  accounts: () => ['accounts', 'list'] as const,
  accountsRoot: () => ['accounts'] as const,
  accountsScan: () => ['account-scan'] as const,
  signin: (name: string) => ['account-signin', name] as const,
  sessionsRoot: () => ['sessions'] as const,
  sessions: (filter: SessionFilter = {}) => ['sessions', filter] as const,
  session: (id: string) => ['session', id, 'summary'] as const,
  sessionEvents: (id: string) => ['session', id, 'events'] as const,
  run: (id: string) => ['run', id, 'view'] as const,
  runEvents: (id: string, type?: EventType) => (type ? (['run', id, 'events', type] as const) : (['run', id, 'events'] as const)),
  transcript: (id: string) => ['run', id, 'transcript'] as const,
  runRoute: (id: string) => ['run', id, 'route'] as const,
  runMessages: (id: string) => ['run', id, 'messages'] as const,
  choices: () => ['choices'] as const,
  models: () => ['models'] as const,
  routingRecords: () => ['routing', 'records'] as const,
  worktrees: () => ['worktrees'] as const,
  agents: () => ['agents'] as const,
  agent: (name: string) => ['agent', name] as const,
  workflows: () => ['workflows'] as const,
  workflowRuns: () => ['workflow-runs'] as const,
  workflowRun: (id: string) => ['workflow-run', id] as const,
  config: () => ['config'] as const,
}

export interface SessionFilter {
  status?: SessionStatus
  q?: string
}


