import { useCallback, useEffect, useRef, useState } from 'react'
import { useParams } from 'react-router'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { endpoints } from '@/api/endpoints'
import { qk, usePollingInterval, useRun } from '@/api/queries'
import type { Event, Page, TranscriptLine } from '@/api/types'

const isSessionEvents = (key: readonly unknown[], sid: string) => key[0] === 'session' && key[1] === sid && key[2] === 'events'

/**
 * Every event of a run (design 4.9: shares the session cache). Inside a session route this reads the session events
 * pages from the query cache (the timeline fills it, and `api/stream.ts` appends live events to it) and filters by
 * run; if the cache is empty it pages the session events into that same cache. Without a session (panel opened
 * elsewhere) it pages `/runs/:id/events`. The key sits under `qk.runEvents(id)`, so the join audit event and other
 * invalidations of that prefix refetch it, as do run view changes and appends to the session cache.
 */
export function useAllRunEvents(runId: string) {
  const qc = useQueryClient()
  const { sessionId } = useParams()
  const run = useRun(runId)
  const refetchInterval = usePollingInterval()
  const key = [...qk.runEvents(runId), 'all', sessionId ?? null]
  const q = useQuery({
    queryKey: key,
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
            after = page.next_cursor
          } while (after)
          items = cached()
        }
        const seen = new Set<string>()
        return items
          .filter((e) => e.run_id === runId && !seen.has(e.id) && seen.add(e.id))
          .sort((a, b) => Date.parse(a.ts) - Date.parse(b.ts) || (a.id < b.id ? -1 : 1))
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

  const refetchAll = () => void qc.invalidateQueries({ queryKey: [...qk.runEvents(runId), 'all'] })
  const seenUpdate = useRef(run.dataUpdatedAt)
  useEffect(() => {
    if (run.dataUpdatedAt !== seenUpdate.current) {
      seenUpdate.current = run.dataUpdatedAt
      refetchAll()
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [run.dataUpdatedAt])
  // Live events land in the session cache: pick them up without re-paging.
  useEffect(() => {
    if (!sessionId) return
    return qc.getQueryCache().subscribe((ev) => {
      if (ev.type === 'updated' && ev.action.type === 'success' && isSessionEvents(ev.query.queryKey, sessionId)) refetchAll()
    })
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [qc, sessionId, runId])
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

/**
 * Transcript lines, loaded page by page from offset 0 until the end (the API only pages forward), then, while the
 * run is active, polled from the tail every 2 s (the stream does not push transcript lines).
 */
export function useTranscriptLines(runId: string, active: boolean) {
  const [state, setState] = useState<TranscriptState>({ lines: [], next: 0, tail: 0, loading: true, error: null })
  const busy = useRef(false)

  const fetchPage = useCallback(
    async (offset: number) => {
      if (busy.current) return
      busy.current = true
      try {
        const page = await endpoints.getTranscript(runId, { offset, limit: 1000 })
        setState((s) => ({
          lines: page.lines.length ? s.lines.concat(page.lines.filter((l) => !s.lines.length || l.offset > s.lines[s.lines.length - 1].offset)) : s.lines,
          next: page.next_offset,
          tail: page.total_bytes,
          loading: false,
          error: null,
        }))
      } catch (error) {
        setState((s) => ({ ...s, loading: false, error, next: s.next }))
      } finally {
        busy.current = false
      }
    },
    [runId],
  )

  const { next, error } = state
  // Keep loading forward until the end.
  useEffect(() => {
    if (next === null || error) return
    void fetchPage(next)
  }, [next, error, fetchPage, state.lines.length])

  // Tail polling while active.
  const { tail } = state
  useEffect(() => {
    if (!active || next !== null) return
    const id = setInterval(() => void fetchPage(tail), 2000)
    return () => clearInterval(id)
  }, [active, next, tail, fetchPage])

  const retry = useCallback(() => {
    setState((s) => ({ ...s, error: null, loading: true }))
    void fetchPage(state.next ?? state.tail)
  }, [fetchPage, state.next, state.tail])

  return { ...state, retry }
}
