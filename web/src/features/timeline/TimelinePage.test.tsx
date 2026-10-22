import { act, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { Route, Routes, useLocation } from 'react-router'
import { describe, expect, it, vi } from 'vitest'
import { qk } from '@/api/keys'
import type { Event, Page } from '@/api/types'
import { server } from '@/test/server'
import { renderWithProviders } from '@/test/render'
import { LARGE_SESSION_ID } from '@/test/fixtures/large'
import { TimelinePage, type TimelinePageProps } from './TimelinePage'

function Where() {
  return <output data-testid="where">{useLocation().pathname}</output>
}
function renderPage(route: string, props: TimelinePageProps = {}) {
  return renderWithProviders(
    <>
      <Routes>
        <Route path="/sessions/:sessionId" element={<TimelinePage {...props} />} />
        <Route path="/sessions/:sessionId/runs/:runId" element={<TimelinePage {...props} />} />
      </Routes>
      <Where />
    </>,
    { route },
  )
}

describe('TimelinePage', () => {
  it('loads the session and its events and shows a lane per run', async () => {
    renderPage('/sessions/sess-1')
    expect(await screen.findByRole('application', { name: /Session timeline/ })).toBeInTheDocument()
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

  it('renders a stub panel by default', async () => {
    renderPage('/sessions/sess-1/runs/run-1')
    expect(await screen.findByText('The run panel is not loaded.')).toBeInTheDocument()
  })

  it('appends live events from the events cache through the rAF buffer', async () => {
    const { queryClient } = renderPage('/sessions/sess-1')
    await screen.findByRole('application')
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
    act(() => queryClient.setQueryData<Page<Event>>(key, { ...page, items: [...page.items, live] }))
    await new Promise((r) => setTimeout(r, 50))
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
  }, 30000)

  it('fits, zooms and turns live off when the user pans', async () => {
    renderPage('/sessions/sess-1')
    await screen.findByRole('application')
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
