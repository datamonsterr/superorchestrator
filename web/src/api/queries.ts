import {
  keepPreviousData,
  useInfiniteQuery,
  useMutation,
  useQuery,
  useQueryClient,
  type QueryKey,
} from '@tanstack/react-query'
import { endpoints } from './endpoints'
import { qk, type SessionFilter } from './keys'
import { useStreamState } from './stream'

export { qk, type SessionFilter }
import type { LevelsPut, ConfigPatch, PruneRequest, ModelCatalog, EventType, Harness, RouteParams, SendMessageParams } from './types'

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
/** Polls every 2 s only while the run is active and the stream is down (the stream pushes events, not transcript lines). */
export function useTranscript(id: string, offset = 0, opts: { active?: boolean } = {}) {
  const streamDown = useStreamState().polling
  return useQuery({
    queryKey: [...qk.transcript(id), offset],
    queryFn: ({ signal }) => endpoints.getTranscript(id, { offset, limit: 500 }, { signal }),
    enabled: !!id,
    refetchInterval: opts.active && streamDown ? 2000 : false,
  })
}
export function useRunRoute(id: string) {
  return useQuery({ queryKey: qk.runRoute(id), queryFn: ({ signal }) => endpoints.getRunRoute(id, { signal }), enabled: !!id })
}
export function useRunMessages(id: string) {
  return useQuery({ queryKey: qk.runMessages(id), queryFn: ({ signal }) => endpoints.listRunMessages(id, { signal }), enabled: !!id })
}

export function useLevels() {
  return useQuery({ queryKey: qk.levels(), queryFn: ({ signal }) => endpoints.getLevels({ signal }) })
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

// ---- Mutations ----
// Variables are named objects. onSuccess returns the invalidations so `mutateAsync`
// resolves after the affected queries have refetched. Failures toast globally
// (MutationCache.onError in lib/query-client.ts; set `meta: { silent: true }` to opt out).

function useApiMutation<V, D>(fn: (v: V) => Promise<D>, invalidate: (v: V, d: D) => QueryKey[] = () => []) {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: fn,
    onSuccess: (data, vars) => Promise.all(invalidate(vars, data).map((queryKey) => qc.invalidateQueries({ queryKey }))),
  })
}

const ACCOUNT_KEYS = () => [qk.accounts(), qk.harnesses(), qk.dashboard()]
type Id = { id: string }
type Name = { name: string }

export const useSendMessage = () =>
  useApiMutation(({ id, body }: Id & { body: SendMessageParams }) => endpoints.sendMessage(id, body), ({ id }) => [qk.run(id), qk.runMessages(id)])
export const useCancelRun = () => useApiMutation(({ id }: Id) => endpoints.cancelRun(id), ({ id }) => [qk.run(id), qk.sessionsRoot()])
export const useAcceptRun = () => useApiMutation(({ id }: Id) => endpoints.acceptRun(id), ({ id }) => [qk.run(id), qk.sessionsRoot()])
export const useRejectRun = () =>
  useApiMutation(({ id, feedback }: Id & { feedback: string }) => endpoints.rejectRun(id, feedback), ({ id }) => [qk.run(id), qk.sessionsRoot()])
/** Records a `join_command_issued` audit event server side, so the run's events are stale afterwards. */
export const useJoinRun = () => useApiMutation(({ id }: Id) => endpoints.joinRun(id), ({ id }) => [qk.runEvents(id)])
export const useRoutePreview = () => useApiMutation((params: RouteParams) => endpoints.route(params))
/** The PUT response is the server's view of the levels, so it goes straight into the cache. */
export function usePutLevels() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (body: LevelsPut) => endpoints.putLevels(body),
    onSuccess: (data) => {
      qc.setQueryData(qk.levels(), data)
      return qc.invalidateQueries({ queryKey: qk.harnesses() })
    },
  })
}
export const useRefreshModels = () => useApiMutation<void, ModelCatalog>(() => endpoints.refreshModels(), () => [qk.models()])
export const useCreateAccount = () =>
  useApiMutation((v: { harness: Harness; name: string; home?: string }) => endpoints.createAccount(v), ACCOUNT_KEYS)
export const useImportAccount = () =>
  useApiMutation((v: { harness: Harness; name: string; home: string }) => endpoints.importAccount(v), ACCOUNT_KEYS)
export const usePatchAccount = () =>
  useApiMutation(
    ({ name, body }: Name & { body: Parameters<typeof endpoints.patchAccount>[1] }) => endpoints.patchAccount(name, body),
    ACCOUNT_KEYS,
  )
export const useRemoveAccount = () =>
  useApiMutation(({ name, purge }: Name & { purge?: boolean }) => endpoints.removeAccount(name, purge), ACCOUNT_KEYS)
export const useFlagAccount = () =>
  useApiMutation(
    ({ name, body }: Name & { body: Parameters<typeof endpoints.flagAccount>[1] }) => endpoints.flagAccount(name, body),
    ACCOUNT_KEYS,
  )
export const useStartLogin = () => useApiMutation(({ name }: Name) => endpoints.startLogin(name), ({ name }) => [qk.signin(name)])
export const usePruneWorktrees = () =>
  useApiMutation((v: PruneRequest | void) => endpoints.pruneWorktrees(v ?? {}), () => [qk.worktrees()])
export const useCreateAgent = () => useApiMutation((v: { name: string; source: string }) => endpoints.createAgent(v), () => [qk.agents()])
export const usePutAgent = () =>
  useApiMutation(({ name, source }: Name & { source: string }) => endpoints.putAgent(name, { source }), ({ name }) => [qk.agents(), qk.agent(name)])
export const useDeleteAgent = () => useApiMutation(({ name }: Name) => endpoints.deleteAgent(name), () => [qk.agents()])
export const useRunWorkflow = () =>
  useApiMutation(({ name, args }: Name & { args?: unknown }) => endpoints.runWorkflow(name, args), () => [qk.workflowRuns()])
export const usePatchConfig = () => useApiMutation((body: ConfigPatch) => endpoints.patchConfig(body), () => [qk.config(), qk.status()])
