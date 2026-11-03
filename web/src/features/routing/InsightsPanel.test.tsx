import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { describe, expect, it } from 'vitest'
import { server } from '@/test/server'
import { renderWithProviders } from '@/test/render'
import * as fx from '@/api/msw/fixtures'
import type { InsightsReport, LevelsPut } from '@/api/types'
import { RoutingPage } from './RoutingPage'

const insightsRegion = () => screen.findByRole('region', { name: 'Insights' })
const levelRegion = (panel: HTMLElement, name: string) => within(panel).getByRole('region', { name: `${name} level insights` })
/** The value next to a label in a level's stats list. */
const stat = (region: HTMLElement, label: string) => within(region).getByText(label).closest('div') as HTMLElement

function capturePut(extra: Record<string, unknown> = {}) {
  const puts: LevelsPut[] = []
  server.use(
    http.put('/api/v1/levels', async ({ request }) => {
      const b = (await request.json()) as LevelsPut
      puts.push(b)
      return HttpResponse.json({ ...fx.levels, ...b, ...extra })
    }),
  )
  return puts
}

async function openApply(panel: HTMLElement) {
  await userEvent.click(await within(panel).findByRole('button', { name: /^Apply suggestion / }))
  const dialog = await screen.findByRole('alertdialog')
  const confirm = within(dialog).getByRole('button', { name: 'Apply suggestion' })
  await waitFor(() => expect(confirm).toBeEnabled())
  return confirm
}

describe('Insights panel', () => {
  it('shows the header, per-level stats, model table and labelled examples', async () => {
    renderWithProviders(<RoutingPage />, { route: '/routing' })
    const panel = await insightsRegion()
    expect(await within(panel).findByText(/12 history runs/)).toBeInTheDocument()

    const complex = levelRegion(panel, 'complex')
    expect(within(complex).getByText('5 runs')).toBeInTheDocument()
    expect(stat(complex, 'success')).toHaveTextContent('60%')
    expect(stat(complex, 'median complexity')).toHaveTextContent('72')
    expect(stat(complex, 'complexity range')).toHaveTextContent('64–85')
    expect(within(complex).getByRole('list', { name: 'Complexity distribution of complex' }).children).toHaveLength(3)
    const table = within(complex).getByRole('table', { name: 'Models of complex by outcome' })
    expect(within(table).getByText('agy/claude-sonnet-4-6/thinking')).toBeInTheDocument()
    const ex = within(within(complex).getByRole('list', { name: 'Example tasks of complex' })).getByRole('listitem')
    for (const t of ['history', 'c=74', 'rejected', 'borderline', 'Refactor the session store to stream events']) expect(within(ex).getByText(t)).toBeInTheDocument()

    const simple = levelRegion(panel, 'simple')
    expect(within(simple).getByText(/No history for this level yet/)).toBeInTheDocument()
    expect(within(simple).getByText('eval')).toBeInTheDocument()
    expect(within(simple).getByText('c=30')).toBeInTheDocument()
  })

  it('says so when there is no history at all', async () => {
    const empty: InsightsReport = { ...fx.insights, history_runs: 0, levels: fx.insights.levels.filter((l) => l.name === 'simple'), suggestions: [] }
    server.use(http.get('/api/v1/routing/insights', () => HttpResponse.json(empty)))
    renderWithProviders(<RoutingPage />, { route: '/routing' })
    const panel = await insightsRegion()
    expect(await within(panel).findByText(/No run history yet/)).toBeInTheDocument()
    expect(within(panel).getByText('eval')).toBeInTheDocument()
    expect(within(panel).getByText(/No suggestions/)).toBeInTheDocument()
  })

  it('applies a suggestion after confirmation with the suggested levels and the current instruction', async () => {
    const puts = capturePut({ restart_required: true })
    renderWithProviders(<RoutingPage />, { route: '/routing' })
    const panel = await insightsRegion()
    const s = fx.insights.suggestions[0]
    expect(await within(panel).findByText('promote_model')).toBeInTheDocument()
    expect(within(panel).getByText(`${s.diff[0].level}.${s.diff[0].field}: ${s.diff[0].before} -> ${s.diff[0].after}`)).toBeInTheDocument()

    const confirm = await openApply(panel)
    expect(puts).toHaveLength(0)
    await userEvent.click(confirm)
    await waitFor(() => expect(puts).toHaveLength(1))
    expect(puts[0].levels).toEqual(s.levels)
    expect(puts[0].routing_instruction).toBe(fx.levels.routing_instruction)
  })

  it('sends one PUT on a double click', async () => {
    const puts = capturePut()
    renderWithProviders(<RoutingPage />, { route: '/routing' })
    const confirm = await openApply(await insightsRegion())
    await userEvent.dblClick(confirm)
    await waitFor(() => expect(puts).toHaveLength(1))
    await new Promise((r) => setTimeout(r, 50))
    expect(puts).toHaveLength(1)
  })

  it('refuses a stale suggestion, refetches insights and sends no PUT', async () => {
    const puts = capturePut()
    let insightGets = 0
    server.use(http.get('/api/v1/routing/insights', () => (insightGets++, HttpResponse.json(fx.insights))))
    const changed = { ...fx.levels, levels: fx.levels.levels.map((l) => (l.name === 'complex' ? { ...l, models: ['codex/gpt-6.1-sol/high'] } : l)) }
    server.use(http.get('/api/v1/levels', () => HttpResponse.json(changed)))
    renderWithProviders(<RoutingPage />, { route: '/routing' })
    const panel = await insightsRegion()
    const confirm = await openApply(panel)
    const before = insightGets
    await userEvent.click(confirm)
    expect(await within(panel).findByText(/Levels changed since this suggestion was made; refresh insights/)).toBeInTheDocument()
    await waitFor(() => expect(insightGets).toBeGreaterThan(before))
    expect(puts).toHaveLength(0)
  })

  it('explains in the dialog when the current levels could not be loaded', async () => {
    server.use(http.get('/api/v1/levels', () => HttpResponse.json({ error: { code: -32004, message: 'down' } }, { status: 500 })))
    renderWithProviders(<RoutingPage />, { route: '/routing' })
    const panel = await insightsRegion()
    await userEvent.click(await within(panel).findByRole('button', { name: /^Apply suggestion / }))
    const dialog = await screen.findByRole('alertdialog')
    expect(within(dialog).getByText(/current levels could not be loaded/)).toBeInTheDocument()
    expect(within(dialog).getByRole('button', { name: 'Apply suggestion' })).toBeDisabled()
  })

  it('shows the API error when applying fails', async () => {
    server.use(http.put('/api/v1/levels', () => HttpResponse.json({ error: { code: -32602, message: 'invalid levels' } }, { status: 400 })))
    renderWithProviders(<RoutingPage />, { route: '/routing' })
    const panel = await insightsRegion()
    await userEvent.click(await openApply(panel))
    expect(await within(panel).findByText('Could not apply the suggestion')).toBeInTheDocument()
    expect(within(panel).getByText(/invalid levels/)).toBeInTheDocument()
  })

  it('shows an error when insights cannot load', async () => {
    server.use(http.get('/api/v1/routing/insights', () => HttpResponse.json({ error: { code: -32004, message: 'down' } }, { status: 500 })))
    renderWithProviders(<RoutingPage />, { route: '/routing' })
    expect(await screen.findByText('Could not load insights')).toBeInTheDocument()
  })

  it('marks models that are in no level in the models panel', async () => {
    renderWithProviders(<RoutingPage />, { route: '/routing' })
    await userEvent.click(await screen.findByRole('button', { name: /Models \(/ }))
    const codex = await screen.findByRole('region', { name: 'codex models' })
    expect(await within(codex).findByText('not in any level')).toBeInTheDocument()
    expect(within(await screen.findByRole('region', { name: 'agy models' })).queryByText('not in any level')).not.toBeInTheDocument()
  })
})
