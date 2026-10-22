import { act, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { useLocation, useNavigate } from 'react-router'
import { describe, expect, it } from 'vitest'
import { server } from '@/test/server'
import { renderWithProviders } from '@/test/render'
import { useEventStream } from '@/api/stream'
import * as fx from '@/api/msw/fixtures'
import type { SessionSummary } from '@/api/types'
import { SessionsPage } from './SessionsPage'

function Loc() {
  const l = useLocation()
  return <output data-testid="loc">{l.search}</output>
}
function Nav() {
  const navigate = useNavigate()
  return (
    <>
      <button onClick={() => navigate('/sessions?q=other')}>go-other</button>
      <button onClick={() => navigate(-1)}>go-back</button>
    </>
  )
}
const ui = (
  <>
    <SessionsPage />
    <Loc />
    <Nav />
  </>
)

const loaded = () => screen.findAllByRole('link', { name: /Open session/ })

describe('SessionsPage', () => {
  it('lists sessions with status badges and open links', async () => {
    renderWithProviders(ui, { route: '/sessions' })
    await loaded()
    const table = screen.getByRole('table')
    expect(within(table).getAllByRole('row')).toHaveLength(4)
    expect(within(table).getByText('Failed')).toBeInTheDocument()
    expect(screen.getByRole('link', { name: /Open session claude-code sess-1/ })).toHaveAttribute('href', '/sessions/sess-1')
  })

  it('mirrors the status filter in the URL and filters the list', async () => {
    renderWithProviders(ui, { route: '/sessions' })
    await loaded()
    await userEvent.click(screen.getByRole('radio', { name: 'Failed' }))
    expect(screen.getByTestId('loc')).toHaveTextContent('?status=failed')
    await waitFor(() => expect(screen.getAllByRole('row')).toHaveLength(2))
  })

  it('reads the filters from the URL on load', async () => {
    renderWithProviders(ui, { route: '/sessions?status=active&q=claude' })
    expect(screen.getByRole('radio', { name: 'Active' })).toBeChecked()
    expect(screen.getByRole('searchbox', { name: 'Search sessions' })).toHaveValue('claude')
    await waitFor(() => expect(screen.getAllByRole('row')).toHaveLength(2))
  })

  it('writes the search text to the URL (debounced)', async () => {
    renderWithProviders(ui, { route: '/sessions' })
    await loaded()
    await userEvent.type(screen.getByRole('searchbox'), 'other')
    await waitFor(() => expect(screen.getByTestId('loc')).toHaveTextContent('?q=other'))
    await waitFor(() => expect(screen.getAllByRole('row')).toHaveLength(2))
  })

  it('re-syncs the search box when ?q= changes externally, without clobbering typing', async () => {
    renderWithProviders(ui, { route: '/sessions?q=claude' })
    const box = screen.getByRole('searchbox', { name: 'Search sessions' })
    await userEvent.click(screen.getByRole('button', { name: 'go-other' }))
    await waitFor(() => expect(box).toHaveValue('other'))
    await userEvent.click(screen.getByRole('button', { name: 'go-back' }))
    await waitFor(() => expect(box).toHaveValue('claude'))
    // typing in progress survives the debounce write-back
    await userEvent.type(box, 'x')
    await new Promise((r) => setTimeout(r, 400))
    expect(box).toHaveValue('claudex')
  })

  it('loads more pages', async () => {
    const many: SessionSummary[] = Array.from({ length: 60 }, (_, i) => ({ ...fx.sessions[0], id: `s-${i}`, client: `client-${i}` }))
    server.use(
      http.get('/api/v1/sessions', ({ request }) => {
        const cursor = Number(new URL(request.url).searchParams.get('cursor') ?? 0)
        const items = many.slice(cursor, cursor + 50)
        return HttpResponse.json({ items, next_cursor: cursor + 50 < many.length ? String(cursor + 50) : null })
      }),
    )
    renderWithProviders(ui, { route: '/sessions' })
    await loaded()
    expect(screen.getAllByRole('row')).toHaveLength(51)
    await userEvent.click(screen.getByRole('button', { name: 'Load more' }))
    await waitFor(() => expect(screen.getAllByRole('row')).toHaveLength(61))
    expect(screen.queryByRole('button', { name: 'Load more' })).not.toBeInTheDocument()
  })

  it('shows the install command when there are no sessions', async () => {
    server.use(http.get('/api/v1/sessions', () => HttpResponse.json({ items: [], next_cursor: null })))
    renderWithProviders(ui, { route: '/sessions' })
    expect(await screen.findByText('No sessions yet')).toBeInTheDocument()
    expect(screen.getByText('superorchestrator install')).toBeInTheDocument()
  })

  it('offers to clear filters when nothing matches', async () => {
    server.use(http.get('/api/v1/sessions', () => HttpResponse.json({ items: [], next_cursor: null })))
    renderWithProviders(ui, { route: '/sessions?status=failed' })
    await userEvent.click(await screen.findByRole('button', { name: 'Clear filters' }))
    expect(screen.getByTestId('loc')).toHaveTextContent('')
  })

  it('shows an error with retry', async () => {
    let fail = true
    server.use(
      http.get('/api/v1/sessions', () =>
        fail ? HttpResponse.json({ error: { code: -32004, message: 'unavailable' } }, { status: 503 }) : HttpResponse.json({ items: fx.sessions, next_cursor: null }),
      ),
    )
    renderWithProviders(ui, { route: '/sessions' })
    expect(await screen.findByRole('alert')).toHaveTextContent('unavailable')
    fail = false
    await userEvent.click(screen.getByRole('button', { name: 'Retry' }))
    expect(await loaded()).not.toHaveLength(0)
  })

  it('updates rows when a session event arrives on the stream', async () => {
    const listeners = new Map<string, (e: MessageEvent) => void>()
    const source = {
      readyState: 1,
      onopen: null,
      onerror: null,
      addEventListener: (t: string, l: (e: MessageEvent) => void) => listeners.set(t, l),
      close() {},
    }
    function Host() {
      useEventStream({ createSource: () => source })
      return <SessionsPage />
    }
    renderWithProviders(<Host />, { route: '/sessions' })
    await loaded()
    const patched = { ...fx.sessions[0], status: 'escalated' as const }
    server.use(http.get('/api/v1/sessions', () => HttpResponse.json({ items: [patched, ...fx.sessions.slice(1)], next_cursor: null })))
    act(() => listeners.get('session')!(new MessageEvent('session', { data: JSON.stringify(patched), lastEventId: '1' })))
    await waitFor(() => expect(screen.getByText('Escalated')).toBeInTheDocument())
  })
})
