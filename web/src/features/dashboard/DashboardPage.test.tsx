import { act, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { describe, expect, it } from 'vitest'
import { server } from '@/test/server'
import { renderWithProviders } from '@/test/render'
import { useEventStream } from '@/api/stream'
import * as fx from '@/api/msw/fixtures'
import type { DailyUsage, DashboardView, Headroom } from '@/api/types'
import { DashboardPage } from './DashboardPage'
import { chartRows } from './usage'

class FakeSource {
  readyState = 1
  onopen: ((e: unknown) => void) | null = null
  onerror: ((e: unknown) => void) | null = null
  listeners = new Map<string, ((e: MessageEvent) => void)[]>()
  addEventListener(t: string, l: (e: MessageEvent) => void) {
    this.listeners.set(t, [...(this.listeners.get(t) ?? []), l])
  }
  close() {}
  emit(name: string, data: unknown, id: string) {
    for (const l of this.listeners.get(name) ?? []) l(new MessageEvent(name, { data: JSON.stringify(data), lastEventId: id }))
  }
}

function Host({ source }: { source: FakeSource }) {
  useEventStream({ createSource: () => source })
  return <DashboardPage />
}

const hr = (pct: number | null, status: Headroom['status'] = 'ok'): Headroom => ({
  ...fx.headroom('codex-acc1', 'main', pct, 50),
  status,
})

describe('DashboardPage', () => {
  it('shows stat cards, accounts, live runs and escalations', async () => {
    renderWithProviders(<DashboardPage />)
    const overview = await screen.findByRole('region', { name: 'Overview' })
    expect(within(overview).getByText('Live runs').nextElementSibling).toHaveTextContent('2')
    expect(within(overview).getByText('Escalations').nextElementSibling).toHaveTextContent('1')
    expect(within(overview).getByText('Cost today')).toBeInTheDocument()
    expect(screen.getByRole('link', { name: 'codex-acc1' })).toBeInTheDocument()
    expect(within(screen.getByLabelText('Live runs')).getByText('refactor-api')).toBeInTheDocument()
    expect(within(screen.getByLabelText('Recent escalations')).getByText('stalled after 3 nudges')).toBeInTheDocument()
    expect(await screen.findByRole('img', { name: 'Cost per day' })).toBeInTheDocument()
    expect(await screen.findByRole('table', { name: 'Cost per day' })).toBeInTheDocument()
  })

  it('colours headroom by threshold and renders unknown', async () => {
    const accounts = [
      { ...fx.accounts[0], name: 'a-ok', headroom: [hr(72)] },
      { ...fx.accounts[0], name: 'a-warn', headroom: [hr(20)] },
      { ...fx.accounts[0], name: 'a-crit', headroom: [hr(5)] },
      { ...fx.accounts[0], name: 'a-unk', headroom: [hr(null, 'unknown')] },
    ]
    server.use(http.get('/api/v1/dashboard', () => HttpResponse.json({ ...fx.dashboard, accounts })))
    renderWithProviders(<DashboardPage />)
    await screen.findByRole('link', { name: 'a-ok' })
    const level = (n: string) => document.querySelector(`tr[data-account="${n}"] [data-level]`)?.getAttribute('data-level')
    expect([level('a-ok'), level('a-warn'), level('a-crit'), level('a-unk')]).toEqual(['ok', 'warn', 'critical', 'unknown'])
    expect(screen.getAllByText('unknown').length).toBeGreaterThan(0)
  })

  it('shows empty states', async () => {
    const empty: DashboardView = { ...fx.dashboard, accounts: [], live_runs: [], escalations: [] }
    server.use(http.get('/api/v1/dashboard', () => HttpResponse.json(empty)))
    renderWithProviders(<DashboardPage />)
    expect(await screen.findByText('No accounts yet')).toBeInTheDocument()
    expect(screen.getByText('No active runs')).toBeInTheDocument()
    expect(screen.getByText('No escalations')).toBeInTheDocument()
  })

  it('shows a loading skeleton, then an error with retry', async () => {
    let fail = true
    server.use(
      http.get('/api/v1/dashboard', () =>
        fail ? HttpResponse.json({ error: { code: -32004, message: 'down' } }, { status: 503 }) : HttpResponse.json(fx.dashboard),
      ),
    )
    renderWithProviders(<DashboardPage />)
    expect(screen.getByTestId('dashboard-loading')).toBeInTheDocument()
    expect(await screen.findByRole('alert')).toHaveTextContent('down')
    fail = false
    await userEvent.click(screen.getByRole('button', { name: 'Retry' }))
    expect(await screen.findByRole('region', { name: 'Overview' })).toBeInTheDocument()
  })

  it('patches the accounts table from SSE messages', async () => {
    const source = new FakeSource()
    renderWithProviders(<Host source={source} />)
    const row = async () => (await screen.findByRole('link', { name: 'codex-acc1' })).closest('tr') as HTMLElement
    expect(within(await row()).getAllByRole('progressbar')[0]).toHaveAttribute('data-level', 'ok')

    server.use(
      http.get('/api/v1/dashboard', () =>
        HttpResponse.json({ ...fx.dashboard, accounts: [{ ...fx.accounts[0], headroom: [hr(4)] }, ...fx.accounts.slice(1)] }),
      ),
    )
    act(() => source.emit('headroom', hr(4), '1'))
    await waitFor(async () => expect(within(await row()).getAllByRole('progressbar')[0]).toHaveAttribute('data-level', 'critical'))
  })

  it('refetches live runs when a run changes state', async () => {
    const source = new FakeSource()
    renderWithProviders(<Host source={source} />)
    await screen.findByLabelText('Live runs')
    server.use(http.get('/api/v1/dashboard', () => HttpResponse.json({ ...fx.dashboard, live_runs: [] })))
    act(() =>
      source.emit('event', { id: 'e9', session_id: 'sess-1', run_id: 'run-1', ts: '2026-01-01T12:30:00Z', type: 'run_finished', data: {} }, '1'),
    )
    expect(await screen.findByText('No active runs')).toBeInTheDocument()
  })
})

describe('chartRows', () => {
  const daily: DailyUsage[] = [
    { day: '2026-01-01', input_tokens: 10, output_tokens: 2, cost: 1, by_account: { b: 0.4, a: 0.6 } },
    { day: '2026-01-02', input_tokens: 20, output_tokens: 4, cost: 2, by_account: { a: 2 } },
  ]
  it('stacks cost per account with zero fill', () => {
    const { series, rows } = chartRows(daily, 'cost')
    expect(series).toEqual(['a', 'b'])
    expect(rows[1]).toEqual({ day: '2026-01-02', a: 2, b: 0 })
  })
  it('splits tokens into input and output', () => {
    expect(chartRows(daily, 'tokens').rows[0]).toEqual({ day: '2026-01-01', input: 10, output: 2 })
  })
})
