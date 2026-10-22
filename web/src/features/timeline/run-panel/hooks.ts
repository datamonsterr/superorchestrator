import { useCallback, useEffect, useRef, useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { endpoints } from '@/api/endpoints'
import { qk, usePollingInterval, useRun } from '@/api/queries'
import type { Event, TranscriptLine } from '@/api/types'

/**
 * Every event of a run, following the cursor through pages of 1000. The key sits under `qk.runEvents(id)`, so the
 * join audit event and other invalidations of that prefix refetch it. It also refetches when the run view changes
 * (state and usage events invalidate `qk.run`), which is how new events show up while the stream is up.
 */
export function useAllRunEvents(runId: string) {
  const qc = useQueryClient()
  const run = useRun(runId)
  const refetchInterval = usePollingInterval()
  const q = useQuery({
    queryKey: [...qk.runEvents(runId), 'all'],
    queryFn: async ({ signal }) => {
      const all: Event[] = []
      let after: string | null = null
      do {
        const page: { items: Event[]; next_cursor: string | null } = await endpoints.listRunEvents(runId, { after, limit: 1000 }, { signal })
        all.push(...page.items)
        after = page.next_cursor
      } while (after)
      return all
    },
    enabled: !!runId,
    refetchInterval,
  })
  const seenUpdate = useRef(run.dataUpdatedAt)
  useEffect(() => {
    if (run.dataUpdatedAt !== seenUpdate.current) {
      seenUpdate.current = run.dataUpdatedAt
      void qc.invalidateQueries({ queryKey: [...qk.runEvents(runId), 'all'] })
    }
  }, [run.dataUpdatedAt, qc, runId])
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
