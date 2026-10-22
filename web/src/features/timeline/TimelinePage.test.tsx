import { act, fireEvent, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { Route, Routes, useLocation } from 'react-router'
import { QueryClient } from '@tanstack/react-query'
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest'
import { applyToCache } from '@/api/stream'
import { qk } from '@/api/keys'
import type { Event, Page } from '@/api/types'
import { server } from '@/test/server'
import { renderWithProviders } from '@/test/render'
import { LARGE_SESSION_ID } from '@/test/fixtures/large'
import { TimelinePage, type TimelinePageProps } from './TimelinePage'

function Where() {
  return <output data-testid="where">{useLocation().pathname}</output>
}
// The run panel is a lazy chunk: load it once so no test pays for the first transform.
beforeAll(async () => {
  await import('./run-panel/RunPanel')
})
afterEach(() => {
  vi.useRealTimers()
})

function renderPage(route: string, props: TimelinePageProps = {}, queryClient?: QueryClient) {
  return renderWithProviders(
    <>
      <Routes>
        <Route path="/sessions/:sessionId" element={<TimelinePage {...props} />} />
        <Route path="/sessions/:sessionId/runs/:runId" element={<TimelinePage {...props} />} />
      </Routes>
      <Where />
    </>,
    { route, queryClient },
  )
}

describe('TimelinePage', () => {
  it('loads the session and its events and shows a lane per run', async () => {
    renderPage('/sessions/sess-1')
    expect(await screen.findByRole('group', { name: 'Session timeline' })).toBeInTheDocument()
    const list = screen.getByRole('list', { name: 'Runs' })
    await waitFor(() => expect(within(list).getAllByRole('listitem')).toHaveLength(3))
    expect(within(list).getByText('refactor-api')).toBeInTheDocument()
    expect(screen.getByRole('heading', { level: 1, name: 'Session timeline' })).toBeInTheDocument()
    expect(screen.getByRole('toolbar', { name: 'Timeline controls' })).toBeInTheDocument()
  })

  it('shows an error for an unknown session', async () => {
    renderPage('/sessions/nope')
    expect(await screen.findByText(/session nope not found/)).toBeInTheDocument()
  })

  it('shows an empty state for a session without runs', async () => {
    renderPage('/sessions/sess-2')
    expect(await screen.findByText('No runs in this session yet')).toBeInTheDocument()
  })

  it('selects a run through the route and hosts the panel only through the prop', async () => {
    const renderRunPanel = vi.fn((runId: string, onClose: () => void) => (
      <div data-testid="panel">
        panel for {runId}
        <button onClick={onClose}>close panel</button>
      </div>
    ))
    renderPage('/sessions/sess-1/runs/run-2', { renderRunPanel })
    expect(await screen.findByTestId('panel')).toHaveTextContent('panel for run-2')
    await waitFor(() => expect(screen.getByTestId('label-run-2')).toHaveAttribute('aria-current', 'true'))
    await userEvent.click(screen.getByText('close panel'))
    expect(screen.getByTestId('where')).toHaveTextContent(/^\/sessions\/sess-1$/)
    expect(screen.queryByTestId('panel')).toBeNull()
  })

  it('navigates to the run route when a lane is clicked', async () => {
    renderPage('/sessions/sess-1', { renderRunPanel: (id) => <div data-testid="panel">{id}</div> })
    await userEvent.click(await screen.findByTestId('label-run-1'))
    expect(screen.getByTestId('where')).toHaveTextContent('/sessions/sess-1/runs/run-1')
    expect(await screen.findByTestId('panel')).toHaveTextContent('run-1')
    // The orchestrator lane has no panel.
    await userEvent.click(screen.getByTestId('label-orchestrator'))
    expect(screen.getByTestId('where')).toHaveTextContent('/sessions/sess-1/runs/run-1')
  })

  it('closes the panel with Escape', async () => {
    renderPage('/sessions/sess-1/runs/run-1', { renderRunPanel: () => <div data-testid="panel" /> })
    await screen.findByTestId('panel')
    await userEvent.keyboard('{Escape}')
    expect(screen.getByTestId('where')).toHaveTextContent(/^\/sessions\/sess-1$/)
  })

  it('renders the run panel by default', async () => {
    renderPage('/sessions/sess-1/runs/run-1')
    expect(await screen.findByRole('tab', { name: 'Brief' }, { timeout: 4000 })).toBeInTheDocument()
    expect(await screen.findByRole('heading', { name: 'refactor-api' })).toBeInTheDocument()
  })

  it('appends live events from the events cache through the rAF buffer', async () => {
    const { queryClient } = renderPage('/sessions/sess-1')
    await screen.findByRole('group', { name: 'Session timeline' })
    const table = await screen.findByTestId('timeline-summary')
    await waitFor(() => expect(within(table).getAllByRole('row').length).toBeGreaterThan(3))
    const before = within(table).getAllByRole('row').length
    const key = [...qk.sessionEvents('sess-1'), null]
    const page = queryClient.getQueryData<Page<Event>>(key)!
    const live: Event = { id: 'ev-live-1', session_id: 'sess-1', run_id: 'run-1', ts: new Date(Date.UTC(2026, 0, 1, 12, 20)).toISOString(), type: 'nudge_sent', data: {} }
    // This is what api/stream.ts does for an SSE `event` message.
    act(() => queryClient.setQueryData<Page<Event>>(key, { ...page, items: [...page.items, live] }))
    await waitFor(() => expect(within(screen.getByTestId('timeline-summary')).getAllByRole('row').length).toBe(before + 1))
    expect(within(screen.getByTestId('timeline-summary')).getByText('Nudge sent')).toBeInTheDocument()
    // Re-delivering the same event does not draw it twice.
    vi.useFakeTimers({ toFake: ['setTimeout', 'requestAnimationFrame'] })
    act(() => queryClient.setQueryData<Page<Event>>(key, { ...page, items: [...page.items, live] }))
    await act(async () => {
      vi.advanceTimersByTime(250) // several frames
    })
    vi.useRealTimers()
    expect(within(screen.getByTestId('timeline-summary')).getAllByRole('row').length).toBe(before + 1)
  })

  it('asks for the session again when an event names an unknown run', async () => {
    let sessionFetches = 0
    server.use(
      http.get('/api/v1/sessions/:id', ({ params }) => {
        sessionFetches++
        return HttpResponse.json({ id: params.id, kind: 'orchestrator', client: 'c', cwd: '/', started_at: new Date(Date.UTC(2026, 0, 1, 12)).toISOString(), status: 'active', run_count: 0, cost: 0, runs: [] })
      }),
      http.get('/api/v1/sessions/:id/events', () => HttpResponse.json({ items: [{ id: 'e1', session_id: 'x', run_id: 'ghost', ts: new Date(Date.UTC(2026, 0, 1, 12, 1)).toISOString(), type: 'run_queued', data: {} }], next_cursor: null })),
    )
    renderPage('/sessions/x')
    await waitFor(() => expect(sessionFetches).toBeGreaterThanOrEqual(2))
  })

  it('pages through 10 000 events of the large session and draws all 31 lanes', async () => {
    renderPage(`/sessions/${LARGE_SESSION_ID}`)
    const list = await screen.findByRole('list', { name: 'Runs' })
    await waitFor(() => expect(within(list).getAllByRole('listitem')).toHaveLength(31), { timeout: 8000 })
    await waitFor(() => expect(screen.queryByText(/^Loading events/)).toBeNull(), { timeout: 15000 })
    expect(screen.getByTestId('timeline-summary').querySelectorAll('tr').length).toBeGreaterThan(100)
  }, 15000)

  it('fits, zooms and turns live off when the user pans', async () => {
    renderPage('/sessions/sess-1')
    await screen.findByRole('group', { name: 'Session timeline' })
    const live = screen.getByRole('button', { name: 'Follow live' })
    expect(live).toHaveAttribute('aria-pressed', 'true')
    await userEvent.click(screen.getByRole('button', { name: 'Zoom in' }))
    expect(screen.getByRole('button', { name: 'Follow live' })).toHaveAttribute('aria-pressed', 'false')
    await userEvent.click(screen.getByRole('button', { name: 'Follow live' }))
    expect(screen.getByRole('button', { name: 'Follow live' })).toHaveAttribute('aria-pressed', 'true')
    await userEvent.click(screen.getByRole('button', { name: 'Fit' }))
    expect(screen.getByRole('button', { name: 'Follow live' })).toHaveAttribute('aria-pressed', 'false')
    expect(screen.getByRole('slider', { name: 'Zoom' })).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Legend' })).toBeInTheDocument()
  })

  it('opens the legend with every state and the join marker', async () => {
    renderPage('/sessions/sess-1')
    await userEvent.click(await screen.findByRole('button', { name: 'Legend' }))
    const states = await screen.findAllByRole('list', { name: 'Run states' })
    expect(within(states.at(-1)!).getAllByRole('listitem')).toHaveLength(10)
    expect(within(screen.getAllByRole('list', { name: 'Markers' }).at(-1)!).getByText('Join')).toBeInTheDocument()
  })
})

describe('TimelinePage run switching', () => {
  it('resets the panel when another run is selected: join POSTs for B, B data, only B transcript', async () => {
    vi.spyOn(HTMLElement.prototype, 'clientHeight', 'get').mockReturnValue(340)
    vi.spyOn(HTMLElement.prototype, 'offsetHeight', 'get').mockReturnValue(34)
    const posts: string[] = []
    const line = (text: string) => ({ lines: [{ offset: 0, role: 'assistant', text }], next_offset: null, total_bytes: text.length + 1 })
    server.use(
      http.post('/api/v1/runs/:id/join', ({ params }) => {
        posts.push(String(params.id))
        return HttpResponse.json({ argv: ['tool', String(params.id)], cwd: `/wt/${String(params.id)}`, env_names: [] })
      }),
      http.get('/api/v1/runs/:id/transcript', ({ params }) => HttpResponse.json(line(`only ${String(params.id)}`))),
    )
    renderPage('/sessions/sess-1/runs/run-1')
    await userEvent.click(await screen.findByRole('tab', { name: 'Join' }, { timeout: 4000 }))
    expect(await screen.findByText('cd /wt/run-1 && tool run-1')).toBeInTheDocument()
    await userEvent.click(screen.getByRole('tab', { name: 'Transcript' }))
    expect(await screen.findByText('only run-1')).toBeInTheDocument()

    await userEvent.click(screen.getByTestId('label-run-2'))
    expect(await screen.findByRole('heading', { name: 'write-tests' })).toBeInTheDocument()
    // Tabs reset to Brief; reopen Join and Transcript for B.
    await userEvent.click(screen.getByRole('tab', { name: 'Join' }))
    expect(await screen.findByText('cd /wt/run-2 && tool run-2')).toBeInTheDocument()
    expect(posts).toEqual(['run-1', 'run-2'])
    await userEvent.click(screen.getByRole('tab', { name: 'Transcript' }))
    expect(await screen.findByText('only run-2')).toBeInTheDocument()
    expect(screen.queryByText('only run-1')).toBeNull()
  })

  it('Events tab reads the shared session cache filtered by run (no per-run request)', async () => {
    let runEventFetches = 0
    server.use(http.get('/api/v1/runs/:id/events', () => { runEventFetches++; return HttpResponse.json({ items: [], next_cursor: null }) }))
    renderPage('/sessions/sess-1/runs/run-2')
    await userEvent.click(await screen.findByRole('tab', { name: 'Events' }, { timeout: 4000 }))
    expect(await screen.findByText('3 of 3 events')).toBeInTheDocument() // run-2: queued, stall, escalated
    expect(runEventFetches).toBe(0)
  })
})

describe('TimelinePage live data and robustness', () => {
  const liveEvent = (id: string, run: string, type: Event['type'] = 'nudge_sent'): Event => ({
    id, session_id: 'sess-1', run_id: run, ts: new Date(Date.UTC(2026, 0, 1, 12, 25)).toISOString(), type, data: {},
  })
  const rows = () => within(screen.getByTestId('timeline-summary')).getAllByRole('row').length

  it('keeps receiving live events after the unobserved event pages would have been garbage collected', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 5 * 60_000 }, mutations: { retry: false } } })
    renderPage('/sessions/sess-1', {}, qc)
    await screen.findByRole('group', { name: 'Session timeline' })
    await waitFor(() => expect(rows()).toBeGreaterThan(3))
    const before = rows()
    await act(async () => {
      await vi.advanceTimersByTimeAsync(6 * 60_000)
    })
    expect(qc.getQueryData([...qk.sessionEvents('sess-1'), null])).toBeDefined()
    // What api/stream.ts does for an SSE `event` message.
    act(() => applyToCache(qc, { event: 'event', data: liveEvent('ev-gc-1', 'run-1') }))
    await waitFor(() => expect(rows()).toBe(before + 1))
  })

  it('Events tab refreshes only for appended events of its own run', async () => {
    const { queryClient } = renderPage('/sessions/sess-1/runs/run-2')
    await userEvent.click(await screen.findByRole('tab', { name: 'Events' }))
    expect(await screen.findByText('3 of 3 events')).toBeInTheDocument()
    const invalidate = vi.spyOn(queryClient, 'invalidateQueries')
    const ours = (c: unknown[][]) => c.filter(([f]) => JSON.stringify((f as { queryKey: unknown }).queryKey).includes('"all"'))
    act(() => applyToCache(queryClient, { event: 'event', data: liveEvent('ev-other', 'run-1') }))
    await new Promise((r) => setTimeout(r, 80)) // a few real frames
    expect(ours(invalidate.mock.calls)).toHaveLength(0)
    act(() => applyToCache(queryClient, { event: 'event', data: liveEvent('ev-mine', 'run-2') }))
    expect(await screen.findByText('4 of 4 events')).toBeInTheDocument()
    expect(ours(invalidate.mock.calls)).toHaveLength(1)
  })

  it('retries a failed catch-up with the same cursor instead of reloading', async () => {
    let calls = 0
    const base = (await import('@/api/msw/fixtures')).events
    server.use(
      http.get('/api/v1/sessions/sess-1/events', () => {
        calls++
        if (calls === 2) return HttpResponse.json({ error: { code: -32004, message: 'unavailable' } }, { status: 503 })
        return HttpResponse.json({ items: calls >= 3 ? [...base, liveEvent('ev-after-retry', 'run-1')] : base, next_cursor: null })
      }),
    )
    const { queryClient } = renderPage('/sessions/sess-1')
    await screen.findByRole('group', { name: 'Session timeline' })
    await waitFor(() => expect(rows()).toBeGreaterThan(3))
    const before = rows()
    await act(async () => {
      await queryClient.invalidateQueries({ queryKey: qk.sessionEvents('sess-1') })
    })
    await waitFor(() => expect(calls).toBe(3), { timeout: 4000 })
    await waitFor(() => expect(rows()).toBe(before + 1))
    expect(screen.getByRole('list', { name: 'Runs' })).toBeInTheDocument()
  })

  it('stops pending catch-up work on unmount', async () => {
    const { queryClient, unmount } = renderPage('/sessions/sess-1')
    await screen.findByRole('group', { name: 'Session timeline' })
    await act(async () => {
      void queryClient.invalidateQueries({ queryKey: qk.sessionEvents('sess-1') })
    })
    unmount()
    vi.useFakeTimers()
    expect(() => vi.runAllTimers()).not.toThrow()
    expect(vi.getTimerCount()).toBe(0)
  })

  it('the loading text is static and busy, with no counts in a live region', async () => {
    renderPage(`/sessions/${LARGE_SESSION_ID}`)
    const status = await screen.findByText('Loading events…', {}, { timeout: 8000 })
    expect(status.closest('[role="status"]')).toHaveAttribute('aria-busy', 'true')
    expect(status.closest('[role="status"]')!.textContent).not.toMatch(/\d/)
  })

  it('the default panel closes itself on Escape; the Sheet persists when another lane is picked', async () => {
    renderPage('/sessions/sess-1/runs/run-1')
    expect(await screen.findByRole('heading', { name: 'refactor-api' })).toBeInTheDocument()
    const sheet = document.querySelector('[data-slot="sheet-content"]')!
    await userEvent.click(screen.getByTestId('label-run-2'))
    expect(await screen.findByRole('heading', { name: 'write-tests' })).toBeInTheDocument()
    expect(document.querySelector('[data-slot="sheet-content"]')).toBe(sheet)
    await userEvent.keyboard('{Escape}')
    await waitFor(() => expect(screen.getByTestId('where')).toHaveTextContent(/^\/sessions\/sess-1$/))
  })

  it('a custom panel is closed by Escape, but not when it comes from a nested dialog or was handled', async () => {
    renderPage('/sessions/sess-1/runs/run-1', {
      renderRunPanel: () => (
        <div data-testid="panel">
          <div role="dialog" aria-label="nested">
            <input aria-label="inside" />
          </div>
        </div>
      ),
    })
    const input = await screen.findByLabelText('inside')
    fireEvent.keyDown(input, { key: 'Escape' })
    expect(screen.getByTestId('where')).toHaveTextContent('/sessions/sess-1/runs/run-1')
    const handled = new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true })
    handled.preventDefault()
    document.body.dispatchEvent(handled)
    expect(screen.getByTestId('where')).toHaveTextContent('/sessions/sess-1/runs/run-1')
    fireEvent.keyDown(document.body, { key: 'Escape' })
    expect(screen.getByTestId('where')).toHaveTextContent(/^\/sessions\/sess-1$/)
  })
})
