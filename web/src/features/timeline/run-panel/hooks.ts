import { useCallback, useEffect, useRef, useState } from 'react'
import { useParams } from 'react-router'
import { useQuery, useQueryClient, type QueryClient } from '@tanstack/react-query'
import { endpoints } from '@/api/endpoints'
import { qk, usePollingInterval, useRun } from '@/api/queries'
import type { Event, Page, TranscriptLine } from '@/api/types'
import { compareKeys } from '../applyEvent'
import { useTabActive } from './tabActive'

const isSessionEvents = (key: readonly unknown[], sid: string) => key[0] === 'session' && key[1] === sid && key[2] === 'events'

const pendingRefresh = new WeakMap<QueryClient, Set<string>>()
/** One invalidation per animation frame per run, however many tabs ask (they all read the same query). */
function scheduleRefresh(qc: QueryClient, runId: string) {
  const pending = pendingRefresh.get(qc) ?? new Set<string>()
  pendingRefresh.set(qc, pending)
  if (pending.has(runId)) return
  pending.add(runId)
  requestAnimationFrame(() => {
    pending.delete(runId)
    void qc.invalidateQueries({ queryKey: [...qk.runEvents(runId), 'all'] })
  })
}

/**
 * Every event of a run (design 4.9: shares the session cache). Inside a session route this reads the session events
 * pages from the query cache (the timeline fills it, and `api/stream.ts` appends live events to it) and filters by
 * run; if the cache is empty it pages the session events into that same cache. Without a session (panel opened
 * elsewhere) it pages `/runs/:id/events`. The key sits under `qk.runEvents(id)`, so the join audit event and other
 * invalidations of that prefix refetch it. Appends to the session cache for this run, and run view changes, refresh
 * it, at most once per animation frame, and never while the first load is still in flight.
 */
export function useAllRunEvents(runId: string) {
  const qc = useQueryClient()
  const { sessionId } = useParams()
  const run = useRun(runId)
  const tabActive = useTabActive()
  const polling = usePollingInterval()
  const refetchInterval = tabActive ? polling : false
  const q = useQuery({
    queryKey: [...qk.runEvents(runId), 'all', sessionId ?? null],
    queryFn: async ({ signal }) => {
      if (sessionId) {
        const cached = () => qc.getQueriesData<Page<Event>>({ queryKey: qk.sessionEvents(sessionId) }).flatMap(([, d]) => d?.items ?? [])
        let items = cached()
        if (items.length === 0) {
          let after: string | null = null
          do {
            const cursor: string | null = after
            const page: Page<Event> = await qc.fetchQuery({
              queryKey: [...qk.sessionEvents(sessionId), cursor],
              queryFn: ({ signal: s }) => endpoints.listSessionEvents(sessionId, { after: cursor, limit: 1000 }, { signal: s }),
              staleTime: 0,
            })
            signal.throwIfAborted()
            after = page.next_cursor
          } while (after)
          items = cached()
        }
        const seen = new Set<string>()
        return items
          .filter((e) => e.run_id === runId && !seen.has(e.id) && seen.add(e.id))
          .sort((a, b) => compareKeys({ ts: Date.parse(a.ts), id: a.id }, { ts: Date.parse(b.ts), id: b.id }))
      }
      const all: Event[] = []
      let after: string | null = null
      do {
        const page: Page<Event> = await endpoints.listRunEvents(runId, { after, limit: 1000 }, { signal })
        all.push(...page.items)
        after = page.next_cursor
      } while (after)
      return all
    },
    enabled: !!runId,
    refetchInterval,
  })

  // Refreshes are coalesced to one per animation frame and skipped until the first load has finished.
  const loaded = useRef(false)
  useEffect(() => {
    loaded.current = q.data !== undefined
  }, [q.data])
  const refresh = useCallback(() => {
    if (!loaded.current) return
    scheduleRefresh(qc, runId)
  }, [qc, runId])

  // The run view changed (state and usage events invalidate it): new events probably exist.
  const seenUpdate = useRef(run.dataUpdatedAt)
  useEffect(() => {
    if (!tabActive || run.dataUpdatedAt === seenUpdate.current) return
    seenUpdate.current = run.dataUpdatedAt
    refresh()
  }, [run.dataUpdatedAt, tabActive, refresh])

  // Back on this tab after being away: events may have arrived meanwhile, so refresh once.
  const wasActive = useRef(tabActive)
  useEffect(() => {
    if (tabActive && !wasActive.current) {
      seenUpdate.current = run.dataUpdatedAt
      refresh()
    }
    wasActive.current = tabActive
  }, [tabActive, refresh, run.dataUpdatedAt])

  // Live events land in the session cache: refresh only when one of them belongs to this run.
  useEffect(() => {
    if (!sessionId || !tabActive) return
    // Baseline: how many items each cached page already had, so only appended ones count.
    const counts = new Map<string, number>(
      qc.getQueriesData<Page<Event>>({ queryKey: qk.sessionEvents(sessionId) }).map(([k, d]) => [JSON.stringify(k), d?.items.length ?? 0]),
    )
    return qc.getQueryCache().subscribe((ev) => {
      if (ev.type !== 'updated' || ev.action.type !== 'success' || !isSessionEvents(ev.query.queryKey, sessionId)) return
      const items = (ev.query.state.data as Page<Event> | undefined)?.items
      if (!items) return
      const key = JSON.stringify(ev.query.queryKey)
      const prev = counts.get(key) ?? 0
      counts.set(key, items.length)
      if (items.slice(prev).some((e) => e.run_id === runId)) refresh()
    })
  }, [qc, sessionId, runId, tabActive, refresh])
  return q
}

interface TranscriptState {
  lines: TranscriptLine[]
  /** Offset of the next page, or null when the end was reached. */
  next: number | null
  /** Byte offset to poll from once at the end. */
  tail: number
  loading: boolean
  error: unknown
}

/** Merges a page; returns the same state when nothing changed (an empty tail poll does not re-render). */
function mergePage(s: TranscriptState, page: { lines: TranscriptLine[]; next_offset: number | null; total_bytes: number }): TranscriptState {
  const last = s.lines.length ? s.lines[s.lines.length - 1].offset : -1
  const fresh = page.lines.filter((l) => l.offset > last)
  if (!fresh.length && s.next === page.next_offset && s.tail === page.total_bytes && !s.loading && s.error == null) return s
  return { lines: fresh.length ? s.lines.concat(fresh) : s.lines, next: page.next_offset, tail: page.total_bytes, loading: false, error: null }
}

/**
 * Transcript lines, loaded page by page from offset 0 until the end (the API only pages forward), then, while the
 * run is active, polled from the tail every 2 s (the stream does not push transcript lines). Requests are aborted
 * when the run changes or the panel closes.
 */
export function useTranscriptLines(runId: string, active: boolean) {
  const [state, setState] = useState<TranscriptState>({ lines: [], next: 0, tail: 0, loading: true, error: null })
  const { next, tail, error } = state

  // Keep loading forward until the end.
  useEffect(() => {
    if (next === null || error != null) return
    const ac = new AbortController()
    endpoints
      .getTranscript(runId, { offset: next, limit: 1000 }, { signal: ac.signal })
      .then((page) => setState((s) => mergePage(s, page)))
      .catch((e) => !ac.signal.aborted && setState((s) => ({ ...s, loading: false, error: e })))
    return () => ac.abort()
  }, [runId, next, error])

  // Tail polling while active.
  useEffect(() => {
    if (!active || next !== null) return
    const ac = new AbortController()
    let inFlight = false
    const id = setInterval(() => {
      if (inFlight) return
      inFlight = true
      endpoints
        .getTranscript(runId, { offset: tail, limit: 1000 }, { signal: ac.signal })
        .then((page) => setState((s) => mergePage(s, page)))
        .catch((e) => !ac.signal.aborted && setState((s) => ({ ...s, error: e })))
        .finally(() => (inFlight = false))
    }, 2000)
    return () => {
      clearInterval(id)
      ac.abort()
    }
  }, [runId, active, next, tail])

  const retry = useCallback(() => setState((s) => ({ ...s, error: null, loading: s.lines.length === 0 })), [])
  return { ...state, retry }
}
