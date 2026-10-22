import { act, renderHook } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import type { ReactNode } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi, type MockInstance } from 'vitest'
import { applyToCache, hasGap, invalidationsFor, useEventStream } from './stream'
import type { Event, EventType, Page, RunViewPublic, SseMessage } from './types'

const ev = (type: EventType, run_id: string | null = 'r1', id = 'e1'): Event => ({
  id,
  session_id: 's1',
  run_id,
  ts: '2026-01-01T00:00:00Z',
  type,
  data: {},
})
const msg = (type: EventType, run_id: string | null = 'r1'): SseMessage => ({ event: 'event', data: ev(type, run_id) })

describe('invalidationsFor', () => {
  it.each([
    ['state_changed', [['run', 'r1'], ['sessions'], ['status']]],
    ['run_started', [['run', 'r1'], ['sessions'], ['status']]],
    ['run_finished', [['run', 'r1'], ['sessions'], ['status']]],
    ['run_queued', [['run', 'r1'], ['sessions'], ['status']]],
    ['usage', [['run', 'r1'], ['usage', 'daily']]],
    ['route_decision', [['run', 'r1', 'route'], ['routing', 'records']]],
    ['message_sent', [['run', 'r1', 'messages']]],
    ['question_asked', [['run', 'r1', 'messages']]],
    ['question_answered', [['run', 'r1', 'messages']]],
    ['worktree_created', [['run', 'r1']]],
    ['worktree_removed', [['run', 'r1']]],
    ['escalated', []],
    ['stall_detected', []],
    ['tool_call_started', []],
  ] as [EventType, unknown[]][])('event %s', (type, extra) => {
    expect(invalidationsFor(msg(type))).toEqual([['dashboard'], ...extra])
  })

  it('skips run-scoped keys when the event has no run', () => {
    expect(invalidationsFor(msg('state_changed', null))).toEqual([['dashboard'], ['sessions'], ['status']])
  })

  it('maps headroom, account, session, workflow, run, resync, hello', () => {
    const accounts = [['accounts'], ['harnesses'], ['dashboard']]
    expect(invalidationsFor({ event: 'headroom', data: {} as never })).toEqual(accounts)
    expect(invalidationsFor({ event: 'account', data: {} as never })).toEqual(accounts)
    expect(invalidationsFor({ event: 'session', data: { id: 's1' } as never })).toEqual([['sessions'], ['session', 's1']])
    expect(invalidationsFor({ event: 'workflow', data: { id: 'w1' } as never })).toEqual([['workflow-runs'], ['workflow-run', 'w1']])
    expect(invalidationsFor({ event: 'run', data: {} as never })).toEqual([])
    expect(invalidationsFor({ event: 'resync', data: {} })).toEqual([[]])
    expect(invalidationsFor({ event: 'hello', data: { seq: 1, server_time: '', boot_id: 'b' } })).toEqual([])
  })
})

describe('hasGap', () => {
  it('detects a jump in the sequence', () => {
    expect(hasGap(null, 5)).toBe(false)
    expect(hasGap(4, 5)).toBe(false)
    expect(hasGap(4, 7)).toBe(true)
  })
})

describe('applyToCache', () => {
  it('appends events to the last cached page once and patches runs', () => {
    const qc = new QueryClient()
    const page: Page<Event> = { items: [ev('run_queued', 'r1', 'e0')], next_cursor: null }
    qc.setQueryData(['session', 's1', 'events', null], page)
    qc.setQueryData(['run', 'r1'], { run_id: 'r1', state: 'queued', recent_events: [] } as unknown as RunViewPublic)
    applyToCache(qc, msg('run_started'))
    applyToCache(qc, msg('run_started')) // duplicate id ignored
    expect(qc.getQueryData<Page<Event>>(['session', 's1', 'events', null])?.items).toHaveLength(2)
    applyToCache(qc, { event: 'run', data: { run_id: 'r1', state: 'running' } as never })
    expect(qc.getQueryData<RunViewPublic>(['run', 'r1'])).toMatchObject({ state: 'running', recent_events: [] })
  })
})

class FakeSource {
  static instances: FakeSource[] = []
  readyState = 0
  onopen: ((e: unknown) => void) | null = null
  onerror: ((e: unknown) => void) | null = null
  listeners = new Map<string, (e: MessageEvent) => void>()
  closed = false
  url: string
  constructor(url: string) {
    this.url = url
    FakeSource.instances.push(this)
  }
  addEventListener(t: string, l: (e: MessageEvent) => void) {
    this.listeners.set(t, l)
  }
  close() {
    this.closed = true
    this.readyState = 2
  }
  open() {
    this.readyState = 1
    this.onopen?.({})
  }
  emit(name: string, data: unknown, id = '') {
    this.listeners.get(name)?.({ data: JSON.stringify(data), lastEventId: id } as MessageEvent)
  }
}

describe('useEventStream', () => {
  let qc: QueryClient
  let invalidate: MockInstance<QueryClient['invalidateQueries']>
  const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={qc}>{children}</QueryClientProvider>
  const render = (retryMs = 10) =>
    renderHook(() => useEventStream({ createSource: (u) => new FakeSource(u), retryMs }), { wrapper })

  beforeEach(() => {
    vi.useFakeTimers()
    FakeSource.instances = []
    qc = new QueryClient()
    invalidate = vi.spyOn(qc, 'invalidateQueries')
  })
  afterEach(() => vi.useRealTimers())

  it('reports open, then polling when the source errors', () => {
    const { result } = render()
    expect(result.current).toMatchObject({ status: 'connecting', polling: false })
    act(() => FakeSource.instances[0].open())
    expect(result.current).toMatchObject({ status: 'open', polling: false })
    act(() => FakeSource.instances[0].onerror?.({}))
    expect(result.current).toMatchObject({ status: 'reconnecting', polling: true })
  })

  it('invalidates per message and throttles the dashboard to one per 2 s', () => {
    render()
    const s = FakeSource.instances[0]
    act(() => s.open())
    invalidate.mockClear()
    act(() => s.emit('event', ev('state_changed', 'r1', 'e1'), '1'))
    act(() => s.emit('event', ev('state_changed', 'r1', 'e2'), '2'))
    const keys = () => invalidate.mock.calls.map((c) => JSON.stringify((c[0] as { queryKey: unknown }).queryKey))
    expect(keys().filter((k) => k === '["dashboard"]')).toHaveLength(1)
    expect(keys().filter((k) => k === '["run","r1"]')).toHaveLength(2)
    act(() => void vi.advanceTimersByTime(2100))
    expect(keys().filter((k) => k === '["dashboard"]')).toHaveLength(2) // trailing call
  })

  it('refetches everything on resync, boot_id change, gaps and reopen', () => {
    render()
    const s = FakeSource.instances[0]
    act(() => s.open())
    invalidate.mockClear()
    act(() => s.emit('resync', {}))
    expect(invalidate).toHaveBeenCalledTimes(1)
    expect(invalidate).toHaveBeenLastCalledWith()

    invalidate.mockClear()
    act(() => s.emit('hello', { seq: 1, server_time: 't', boot_id: 'a' }, '1'))
    expect(invalidate).not.toHaveBeenCalled()
    act(() => s.emit('hello', { seq: 9, server_time: 't', boot_id: 'b' }, '9'))
    expect(invalidate).toHaveBeenCalledTimes(1)

    invalidate.mockClear()
    act(() => s.emit('workflow', { id: 'w', name: 'n', state: 'running', ts: 't' }, '10'))
    invalidate.mockClear()
    act(() => s.emit('workflow', { id: 'w', name: 'n', state: 'running', ts: 't' }, '15')) // gap
    expect(invalidate.mock.calls.some((c: unknown[]) => c.length === 0)).toBe(true)

    invalidate.mockClear()
    act(() => s.onerror?.({}))
    act(() => s.open()) // native reconnect
    expect(invalidate).toHaveBeenCalledWith()
  })

  it('recreates a closed source after the retry delay and ignores bad JSON', () => {
    const { result, unmount } = render(50)
    const s = FakeSource.instances[0]
    act(() => s.listeners.get('event')?.({ data: '{nope', lastEventId: '' } as MessageEvent))
    act(() => {
      s.readyState = 2
      s.onerror?.({})
    })
    expect(result.current).toMatchObject({ status: 'closed', polling: true })
    expect(FakeSource.instances).toHaveLength(1)
    act(() => void vi.advanceTimersByTime(60))
    expect(FakeSource.instances).toHaveLength(2)
    unmount()
    expect(FakeSource.instances[1].closed).toBe(true)
  })

  it('falls back to polling when EventSource is unavailable', () => {
    vi.stubGlobal('EventSource', undefined)
    const { result } = renderHook(() => useEventStream(), { wrapper })
    expect(result.current).toMatchObject({ status: 'closed', polling: true })
    vi.unstubAllGlobals()
  })
})
