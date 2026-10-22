import { useCallback, useEffect, useReducer, useRef, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { endpoints } from '@/api/endpoints'
import { qk, useSession } from '@/api/queries'
import type { Event, Page, RunViewPublic } from '@/api/types'
import { applyEvent, withNow } from './applyEvent'
import { deriveTimeline } from './derive'
import type { TimelineModel } from './model'

interface State {
  runs: RunViewPublic[] | null
  /** Raw events, in arrival order; only used to re-derive. */
  events: Event[]
  model: TimelineModel | null
}
type Action =
  | { type: 'runs'; runs: RunViewPublic[]; now: number }
  | { type: 'append'; events: Event[]; now: number }
  | { type: 'tick'; now: number }
  | { type: 'reset'; now: number }

/** Batches larger than this are cheaper to re-derive than to fold. */
const FOLD_LIMIT = 64

export function timelineReducer(s: State, a: Action): State {
  switch (a.type) {
    case 'runs':
      return { runs: a.runs, events: s.events, model: deriveTimeline(a.runs, s.events, a.now) }
    case 'append': {
      if (!s.runs || !s.model || a.events.length === 0) return s
      const events = s.events.concat(a.events)
      let model = s.model
      if (a.events.length > FOLD_LIMIT) model = deriveTimeline(s.runs, events, a.now)
      else {
        for (const e of a.events) model = applyEvent(model, e, a.now)
        if (model.needsRebuild) model = deriveTimeline(s.runs, events, a.now)
      }
      return { ...s, events, model }
    }
    case 'tick':
      return s.model ? { ...s, model: withNow(s.model, a.now) } : s
    case 'reset':
      return { ...s, events: [], model: s.runs ? deriveTimeline(s.runs, [], a.now) : null }
  }
}

const isEventsKey = (key: readonly unknown[], sid: string) => key[0] === 'session' && key[1] === sid && key[2] === 'events'

/**
 * Loads the session and all its events (pages of 1000 following the cursor), derives the model into a reducer and
 * keeps it live: SSE events land in the query cache (see `api/stream.ts`), and this hook picks them up from the
 * cache into a buffer that is flushed once per animation frame (design 4.7).
 */
export function useTimelineModel(sessionId: string) {
  const qc = useQueryClient()
  const session = useSession(sessionId)
  const [now, setNow] = useState(() => Date.now())
  const [state, dispatch] = useReducer(timelineReducer, { runs: null, events: [], model: null })
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<unknown>(null)

  // Runs changed (first load, or a refetch after an unknown run appeared): re-derive.
  const runs = session.data?.runs
  if (runs && state.runs !== runs) dispatch({ type: 'runs', runs, now })
  const ready = state.runs !== null

  const seen = useRef(new Set<string>())
  const buffer = useRef<Event[]>([])
  const frame = useRef<{ kind: 'raf' | 'timeout'; id: number } | null>(null)
  const lastAfter = useRef<string | null>(null)

  const flush = useCallback(() => {
    frame.current = null
    const batch = buffer.current
    buffer.current = []
    if (batch.length) dispatch({ type: 'append', events: batch, now: Date.now() })
  }, [])
  const schedule = useCallback(() => {
    if (frame.current) return
    if (typeof requestAnimationFrame === 'function' && !document.hidden) {
      frame.current = { kind: 'raf', id: requestAnimationFrame(flush) }
    } else frame.current = { kind: 'timeout', id: window.setTimeout(flush, 100) }
  }, [flush])

  /** Queues unseen events. `tailOnly` stops at the first known id (live appends go to the end of the last page). */
  const ingest = useCallback(
    (items: readonly Event[], tailOnly: boolean) => {
      const fresh: Event[] = []
      for (let i = tailOnly ? items.length - 1 : 0; tailOnly ? i >= 0 : i < items.length; tailOnly ? i-- : i++) {
        const e = items[i]
        if (seen.current.has(e.id)) {
          if (tailOnly) break
          continue
        }
        seen.current.add(e.id)
        fresh.push(e)
      }
      if (tailOnly) fresh.reverse()
      if (fresh.length) {
        buffer.current.push(...fresh)
        schedule()
      }
    },
    [schedule],
  )

  /** Pages from cursor `from` to the end. */
  const load = useCallback(
    async (from: string | null, token: { cancelled: boolean }) => {
      let after = from
      for (;;) {
        const page = await qc.fetchQuery<Page<Event>>({
          queryKey: [...qk.sessionEvents(sessionId), after],
          queryFn: ({ signal }) => endpoints.listSessionEvents(sessionId, { after, limit: 1000 }, { signal }),
          staleTime: 0,
        })
        if (token.cancelled) return
        lastAfter.current = after
        ingest(page.items, false)
        if (!page.next_cursor) return
        after = page.next_cursor
      }
    },
    [qc, sessionId, ingest],
  )

  useEffect(() => {
    if (!ready) return
    const token = { cancelled: false }
    load(null, token)
      .catch((e) => !token.cancelled && setError(e))
      .finally(() => !token.cancelled && setLoading(false))
    return () => {
      token.cancelled = true
    }
  }, [ready, load])

  // Live: SSE appends land in the events cache; resync / reconnect invalidates it, which triggers a catch-up.
  useEffect(() => {
    if (!ready) return
    let catching = false
    const token = { cancelled: false }
    const unsubscribe = qc.getQueryCache().subscribe((ev) => {
      if (ev.type !== 'updated' || !isEventsKey(ev.query.queryKey, sessionId)) return
      if (ev.action.type === 'invalidate') {
        if (catching) return
        catching = true
        setTimeout(() => {
          load(lastAfter.current, token)
            .catch(() => {
              // The cursor was rejected: full reload.
              seen.current.clear()
              buffer.current = []
              dispatch({ type: 'reset', now: Date.now() })
              lastAfter.current = null
              return load(null, token)
            })
            .catch((e) => !token.cancelled && setError(e))
            .finally(() => (catching = false))
        }, 0)
        return
      }
      const data = ev.query.state.data as Page<Event> | undefined
      if (data && ev.action.type === 'success') ingest(data.items, true)
    })
    return () => {
      token.cancelled = true
      unsubscribe()
    }
  }, [ready, qc, sessionId, ingest, load])

  useEffect(
    () => () => {
      const f = frame.current
      if (!f) return
      if (f.kind === 'raf') cancelAnimationFrame(f.id)
      else clearTimeout(f.id)
    },
    [],
  )

  // Open segments and the axis advance once a second.
  const active = state.model?.active ?? false
  useEffect(() => {
    if (!active) return
    const id = setInterval(() => {
      const t = Date.now()
      setNow(t)
      dispatch({ type: 'tick', now: t })
    }, 1000)
    return () => clearInterval(id)
  }, [active])

  // A run that events mention but the session does not list yet: refetch the session once per id.
  const asked = useRef(new Set<string>())
  const missing = state.model?.missingRuns
  useEffect(() => {
    const fresh = (missing ?? []).filter((id) => !asked.current.has(id))
    if (!fresh.length) return
    fresh.forEach((id) => asked.current.add(id))
    void qc.invalidateQueries({ queryKey: qk.session(sessionId) })
  }, [missing, qc, sessionId])

  return { model: state.model, session, loadedEvents: state.events.length, loading, error }
}
