import { createContext, useContext, useEffect, useRef, useState } from 'react'
import { useQueryClient, type QueryClient, type QueryKey } from '@tanstack/react-query'
import { API_BASE } from './client'
import type { Event, Page, RunViewPublic, SessionSummary, SseMessage } from './types'

/** Keys that are refetched at most once per interval (design 2.3). */
export const THROTTLE_MS: { key: QueryKey; ms: number }[] = [
  { key: ['dashboard'], ms: 2000 },
  { key: ['usage', 'daily'], ms: 10000 },
]

/**
 * Pure map from a stream message to the query keys it invalidates (design 2.3).
 * `event` messages also append to the session events cache (see `applyToCache`)
 * and `run` messages patch `["run", id]` in place, so those need no refetch.
 */
export function invalidationsFor(msg: SseMessage): QueryKey[] {
  switch (msg.event) {
    case 'event': {
      const ev = msg.data
      const rid = ev.run_id
      const keys: QueryKey[] = [['dashboard']]
      switch (ev.type) {
        case 'state_changed':
        case 'run_started':
        case 'run_finished':
        case 'run_queued':
          if (rid) keys.push(['run', rid])
          keys.push(['sessions'], ['status'])
          break
        case 'usage':
          if (rid) keys.push(['run', rid])
          keys.push(['usage', 'daily'])
          break
        case 'route_decision':
          if (rid) keys.push(['run', rid, 'route'])
          keys.push(['routing', 'records'])
          break
        case 'message_sent':
        case 'question_asked':
        case 'question_answered':
          if (rid) keys.push(['run', rid, 'messages'])
          break
        case 'worktree_created':
        case 'worktree_removed':
          if (rid) keys.push(['run', rid])
          break
        default:
          break
      }
      return keys
    }
    case 'run':
      return [] // patched in the cache
    case 'session':
      return [['sessions'], ['session', msg.data.id]]
    case 'headroom':
    case 'account':
      return [['accounts'], ['harnesses'], ['dashboard']]
    case 'workflow':
      return [['workflow-runs'], ['workflow-run', msg.data.id]]
    case 'resync':
      return [[]] // empty prefix matches every query
    case 'hello':
      return []
  }
}

/** True when the sequence jumped, i.e. events were missed (monotonic `id`s). */
export function hasGap(lastSeq: number | null, id: number): boolean {
  return lastSeq !== null && id > lastSeq + 1
}

/** Appends a stream event to every cached page-0 style `["session", sid, "events", ...]` query. */
function appendToSessionEvents(qc: QueryClient, ev: Event) {
  const queries = qc.getQueriesData<Page<Event>>({ queryKey: ['session', ev.session_id, 'events'] })
  for (const [key, data] of queries) {
    // Only the last page (no next cursor) can receive live events.
    if (!data || data.next_cursor !== null || data.items.some((e) => e.id === ev.id)) continue
    qc.setQueryData<Page<Event>>(key, { ...data, items: [...data.items, ev] })
  }
}

/** Applies the non-invalidation side of a message: cache appends and patches. */
export function applyToCache(qc: QueryClient, msg: SseMessage) {
  if (msg.event === 'event') appendToSessionEvents(qc, msg.data)
  if (msg.event === 'run') {
    const patch = msg.data
    qc.setQueryData<RunViewPublic>(['run', patch.run_id], (old) => (old ? { ...old, ...patch } : old))
  }
  if (msg.event === 'session') {
    const patch: SessionSummary = msg.data
    qc.setQueryData(['session', patch.id], (old: SessionSummary | undefined) => (old ? { ...old, ...patch } : old))
  }
}

export type StreamStatus = 'connecting' | 'open' | 'reconnecting' | 'closed'
export interface StreamState {
  status: StreamStatus
  /** True when the stream is down and queries should poll (design 2.3). */
  polling: boolean
  bootId: string | null
}
const DEFAULT_STATE: StreamState = { status: 'connecting', polling: false, bootId: null }

export const StreamContext = createContext<StreamState>(DEFAULT_STATE)
export const useStreamState = () => useContext(StreamContext)

interface SourceLike {
  readyState: number
  onopen: ((e: unknown) => void) | null
  onerror: ((e: unknown) => void) | null
  addEventListener(type: string, l: (e: MessageEvent) => void): void
  close(): void
}
export interface EventStreamOptions {
  session?: string
  url?: string
  /** Injectable for tests. */
  createSource?: (url: string) => SourceLike
  /** Delay before re-creating a closed source. */
  retryMs?: number
}

const EVENT_NAMES = ['event', 'run', 'session', 'headroom', 'account', 'workflow', 'hello', 'resync'] as const

/**
 * One EventSource for the whole app. The browser reconnects by itself and
 * sends `Last-Event-ID`; if the source is closed for good (non-200, network
 * policy) a new one is created, which cannot resend the id, so that counts as
 * a gap and everything is refetched.
 */
export function useEventStream(opts: EventStreamOptions = {}): StreamState {
  const qc = useQueryClient()
  const [state, setState] = useState<StreamState>(DEFAULT_STATE)
  const { session, retryMs = 3000 } = opts
  const optsRef = useRef(opts)
  useEffect(() => {
    optsRef.current = opts
  })

  useEffect(() => {
    const factory =
      optsRef.current.createSource ??
      (typeof EventSource === 'undefined' ? null : (u: string) => new EventSource(u) as unknown as SourceLike)
    if (!factory) {
      setState({ status: 'closed', polling: true, bootId: null })
      return
    }
    const url = optsRef.current.url ?? `${API_BASE}/stream${session ? `?session=${encodeURIComponent(session)}` : ''}`
    let source: SourceLike | null = null
    let retry: ReturnType<typeof setTimeout> | undefined
    let disposed = false
    let bootId: string | null = null
    let lastSeq: number | null = null
    let everOpened = false
    const timers = new Map<string, { last: number; trailing?: ReturnType<typeof setTimeout> }>()

    const invalidate = (key: QueryKey) => {
      const throttle = THROTTLE_MS.find((t) => JSON.stringify(t.key) === JSON.stringify(key))
      if (!throttle) return void qc.invalidateQueries({ queryKey: key })
      const id = JSON.stringify(key)
      const slot = timers.get(id) ?? { last: 0 }
      const wait = slot.last + throttle.ms - Date.now()
      if (wait <= 0) {
        slot.last = Date.now()
        void qc.invalidateQueries({ queryKey: key })
      } else if (!slot.trailing) {
        slot.trailing = setTimeout(() => {
          slot.trailing = undefined
          slot.last = Date.now()
          void qc.invalidateQueries({ queryKey: key })
        }, wait)
      }
      timers.set(id, slot)
    }
    const invalidateAll = () => void qc.invalidateQueries()

    const handle = (name: string, e: MessageEvent) => {
      let data: unknown
      try {
        data = e.data ? JSON.parse(e.data as string) : {}
      } catch {
        return
      }
      const seq = Number(e.lastEventId)
      if (Number.isFinite(seq) && e.lastEventId !== '') {
        if (name !== 'hello' && hasGap(lastSeq, seq)) invalidateAll()
        lastSeq = seq
      }
      const msg = { event: name, data } as SseMessage
      if (msg.event === 'hello') {
        if (bootId !== null && bootId !== msg.data.boot_id) {
          lastSeq = null
          invalidateAll() // supervisor restarted
        }
        bootId = msg.data.boot_id
        setState((s) => ({ ...s, bootId }))
        return
      }
      if (msg.event === 'resync') {
        invalidateAll()
        return
      }
      applyToCache(qc, msg)
      for (const key of invalidationsFor(msg)) invalidate(key)
    }

    const connect = () => {
      if (disposed) return
      const s = factory(url)
      source = s
      for (const n of EVENT_NAMES) s.addEventListener(n, (e) => handle(n, e))
      s.onopen = () => {
        // Reopened after an outage: refetch what the polling window may have missed.
        if (everOpened) invalidateAll()
        everOpened = true
        setState((st) => ({ ...st, status: 'open', polling: false }))
      }
      s.onerror = () => {
        const closed = s.readyState === 2
        setState((st) => ({ ...st, status: closed ? 'closed' : 'reconnecting', polling: true }))
        if (closed) {
          s.close()
          lastSeq = null
          retry = setTimeout(connect, optsRef.current.retryMs ?? retryMs)
        }
      }
    }
    connect()

    return () => {
      disposed = true
      clearTimeout(retry)
      for (const t of timers.values()) clearTimeout(t.trailing)
      source?.close()
    }
  }, [qc, session, retryMs])

  return state
}
