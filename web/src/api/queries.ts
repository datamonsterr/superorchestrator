import {
  keepPreviousData,
  useInfiniteQuery,
  useMutation,
  useQuery,
  useQueryClient,
  type QueryClient,
  type QueryKey,
} from '@tanstack/react-query'
import { endpoints } from './endpoints'
import { useStreamState } from './stream'
import type { EventType, SessionStatus } from './types'

/** Query key factory (design 2.2). Hooks and `invalidationsFor` share these shapes. */
export const qk = {
  status: () => ['status'] as const,
  dashboard: () => ['dashboard'] as const,
  usageDaily: (days: number) => ['usage', 'daily', days] as const,
  doctor: () => ['doctor'] as const,
  harnesses: () => ['harnesses'] as const,
  accounts: () => ['accounts'] as const,
  accountsScan: () => ['accounts', 'scan'] as const,
  signin: (name: string) => ['accounts', name, 'signin'] as const,
  sessions: (filter: SessionFilter = {}) => ['sessions', filter] as const,
  session: (id: string) => ['session', id] as const,
  sessionEvents: (id: string) => ['session', id, 'events'] as const,
  run: (id: string) => ['run', id] as const,
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

/** 5 s polling while the SSE stream is down (design 2.3), otherwise false. */
export function usePollingInterval(ms = 5000): number | false {
  return useStreamState().polling ? ms : false
}

export function useStatus() {
  return useQuery({ queryKey: qk.status(), queryFn: ({ signal }) => endpoints.getStatus({ signal }), retry: 1 })
}
export function useDashboard() {
  const refetchInterval = usePollingInterval()
  return useQuery({ queryKey: qk.dashboard(), queryFn: ({ signal }) => endpoints.getDashboard({ signal }), refetchInterval })
}
export function useDailyUsage(days = 30) {
  return useQuery({ queryKey: qk.usageDaily(days), queryFn: ({ signal }) => endpoints.getDailyUsage(days, { signal }) })
}
export function useDoctor() {
  return useQuery({ queryKey: qk.doctor(), queryFn: ({ signal }) => endpoints.getDoctor({ signal }) })
}
export function useHarnesses() {
  return useQuery({ queryKey: qk.harnesses(), queryFn: ({ signal }) => endpoints.getHarnesses({ signal }) })
}
export function useAccounts() {
  return useQuery({ queryKey: qk.accounts(), queryFn: ({ signal }) => endpoints.listAccounts({ signal }) })
}
export function useAccountScan(enabled = true) {
  return useQuery({ queryKey: qk.accountsScan(), queryFn: ({ signal }) => endpoints.scanAccounts({ signal }), enabled })
}
/** Polls every 2 s until the sign-in reaches done, expired or failed. */
export function useSignInStatus(name: string, enabled = true) {
  return useQuery({
    queryKey: qk.signin(name),
    queryFn: ({ signal }) => endpoints.getLoginStatus(name, { signal }),
    enabled,
    refetchInterval: (q) => {
      const s = q.state.data?.state
      return s === 'done' || s === 'expired' || s === 'failed' ? false : 2000
    },
  })
}

export function useSessions(filter: SessionFilter = {}, limit = 50) {
  const refetchInterval = usePollingInterval()
  return useInfiniteQuery({
    queryKey: qk.sessions(filter),
    queryFn: ({ pageParam, signal }) => endpoints.listSessions({ ...filter, limit, cursor: pageParam }, { signal }),
    initialPageParam: null as string | null,
    getNextPageParam: (last) => last.next_cursor,
    refetchInterval,
    placeholderData: keepPreviousData,
  })
}
export function useSession(id: string) {
  return useQuery({ queryKey: qk.session(id), queryFn: ({ signal }) => endpoints.getSession(id, { signal }), enabled: !!id })
}
/** One page of events. The timeline pages through `next_cursor` itself. */
export function useSessionEvents(id: string, after?: string | null) {
  return useQuery({
    queryKey: [...qk.sessionEvents(id), after ?? null],
    queryFn: ({ signal }) => endpoints.listSessionEvents(id, { after, limit: 1000 }, { signal }),
    enabled: !!id,
  })
}
export function useRun(id: string) {
  const refetchInterval = usePollingInterval()
  return useQuery({ queryKey: qk.run(id), queryFn: ({ signal }) => endpoints.getRun(id, { signal }), enabled: !!id, refetchInterval })
}
export function useRunEvents(id: string, type?: EventType) {
  return useQuery({
    queryKey: qk.runEvents(id, type),
    queryFn: ({ signal }) => endpoints.listRunEvents(id, { type, limit: 1000 }, { signal }),
    enabled: !!id,
  })
}
export function useTranscript(id: string, offset = 0, opts: { active?: boolean } = {}) {
  return useQuery({
    queryKey: [...qk.transcript(id), offset],
    queryFn: ({ signal }) => endpoints.getTranscript(id, { offset, limit: 500 }, { signal }),
    enabled: !!id,
    refetchInterval: opts.active ? 2000 : false,
  })
}
export function useRunRoute(id: string) {
  return useQuery({ queryKey: qk.runRoute(id), queryFn: ({ signal }) => endpoints.getRunRoute(id, { signal }), enabled: !!id })
}
export function useRunMessages(id: string) {
  return useQuery({ queryKey: qk.runMessages(id), queryFn: ({ signal }) => endpoints.listRunMessages(id, { signal }), enabled: !!id })
}

export function useChoices() {
  return useQuery({ queryKey: qk.choices(), queryFn: ({ signal }) => endpoints.getChoices({ signal }) })
}
export function useModels() {
  return useQuery({ queryKey: qk.models(), queryFn: ({ signal }) => endpoints.getModels({ signal }) })
}
export function useRouteRecords(limit = 50) {
  return useInfiniteQuery({
    queryKey: qk.routingRecords(),
    queryFn: ({ pageParam, signal }) => endpoints.listRouteRecords({ limit, cursor: pageParam }, { signal }),
    initialPageParam: null as string | null,
    getNextPageParam: (last) => last.next_cursor,
  })
}
export function useWorktrees() {
  return useQuery({ queryKey: qk.worktrees(), queryFn: ({ signal }) => endpoints.listWorktrees({ signal }) })
}
export function useAgents() {
  return useQuery({ queryKey: qk.agents(), queryFn: ({ signal }) => endpoints.listAgents({ signal }) })
}
export function useAgent(name: string) {
  return useQuery({ queryKey: qk.agent(name), queryFn: ({ signal }) => endpoints.getAgent(name, { signal }), enabled: !!name })
}
export function useWorkflows() {
  return useQuery({ queryKey: qk.workflows(), queryFn: ({ signal }) => endpoints.listWorkflows({ signal }) })
}
export function useWorkflowRuns(limit = 50) {
  return useInfiniteQuery({
    queryKey: qk.workflowRuns(),
    queryFn: ({ pageParam, signal }) => endpoints.listWorkflowRuns({ limit, cursor: pageParam }, { signal }),
    initialPageParam: null as string | null,
    getNextPageParam: (last) => last.next_cursor,
  })
}
export function useWorkflowRun(id: string) {
  return useQuery({ queryKey: qk.workflowRun(id), queryFn: ({ signal }) => endpoints.getWorkflowRun(id, { signal }), enabled: !!id })
}
export function useConfig() {
  return useQuery({ queryKey: qk.config(), queryFn: ({ signal }) => endpoints.getConfig({ signal }) })
}

// ---- Mutations: each invalidates the keys it affects ----

function useApiMutation<TArgs extends unknown[], TData>(
  fn: (...args: TArgs) => Promise<TData>,
  invalidate: (qc: QueryClient, data: TData, args: TArgs) => QueryKey[] = () => [],
) {
  const qc = useQueryClient()
  return useMutation({
    // useMutation takes a single variable; wrap the args in a tuple.
    mutationFn: (args: TArgs) => fn(...args),
    onSuccess: (data, args) => {
      for (const key of invalidate(qc, data, args)) void qc.invalidateQueries({ queryKey: key })
    },
  })
}
/** Call as `m.mutate([a, b])`; the tuple mirrors the endpoint's arguments. */

export const useSendMessage = () => useApiMutation(endpoints.sendMessage, (_q, _d, [id]) => [qk.run(id), qk.runMessages(id)])
export const useCancelRun = () => useApiMutation(endpoints.cancelRun, (_q, _d, [id]) => [qk.run(id), ['sessions']])
export const useAcceptRun = () => useApiMutation(endpoints.acceptRun, (_q, _d, [id]) => [qk.run(id), ['sessions']])
export const useRejectRun = () => useApiMutation(endpoints.rejectRun, (_q, _d, [id]) => [qk.run(id), ['sessions']])
export const useJoinRun = () => useApiMutation(endpoints.joinRun, (_q, _d, [id]) => [qk.run(id), ['session']])
export const useRoutePreview = () => useApiMutation(endpoints.route)
export const usePutChoices = () => useApiMutation(endpoints.putChoices, () => [qk.choices(), qk.harnesses()])
export const useRefreshModels = () => useApiMutation(endpoints.refreshModels, () => [qk.models()])
export const useCreateAccount = () => useApiMutation(endpoints.createAccount, () => [qk.accounts(), qk.harnesses(), qk.dashboard()])
export const useImportAccount = () => useApiMutation(endpoints.importAccount, () => [qk.accounts(), qk.harnesses(), qk.dashboard()])
export const usePatchAccount = () => useApiMutation(endpoints.patchAccount, () => [qk.accounts(), qk.harnesses(), qk.dashboard()])
export const useRemoveAccount = () => useApiMutation(endpoints.removeAccount, () => [qk.accounts(), qk.harnesses(), qk.dashboard()])
export const useFlagAccount = () => useApiMutation(endpoints.flagAccount, () => [qk.accounts(), qk.harnesses(), qk.dashboard()])
export const useStartLogin = () => useApiMutation(endpoints.startLogin, (_q, _d, [name]) => [qk.signin(name)])
export const usePruneWorktrees = () => useApiMutation(endpoints.pruneWorktrees, () => [qk.worktrees()])
export const useCreateAgent = () => useApiMutation(endpoints.createAgent, () => [qk.agents()])
export const usePutAgent = () => useApiMutation(endpoints.putAgent, (_q, _d, [name]) => [qk.agents(), qk.agent(name)])
export const useDeleteAgent = () => useApiMutation(endpoints.deleteAgent, () => [qk.agents()])
export const useRunWorkflow = () => useApiMutation(endpoints.runWorkflow, () => [qk.workflowRuns()])
export const usePatchConfig = () => useApiMutation(endpoints.patchConfig, () => [qk.config(), qk.status()])
