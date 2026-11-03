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

describe('Insights panel', () => {
  it('shows the header, per-level stats, model table and labelled examples', async () => {
    renderWithProviders(<RoutingPage />, { route: '/routing' })
    const panel = await insightsRegion()
    expect(await within(panel).findByText(/12 history runs/)).toBeInTheDocument()

    const complex = panel.querySelector('[data-insight-level="complex"]') as HTMLElement
    expect(within(complex).getByText('5 runs')).toBeInTheDocument()
    expect(within(complex).getByText('success').nextSibling).toHaveTextContent('60%')
    expect(within(complex).getByText('median complexity').nextSibling).toHaveTextContent('72')
    expect(within(complex).getByText('complexity range').nextSibling).toHaveTextContent('64–85')
    expect(within(complex).getByRole('list', { name: 'Complexity distribution of complex' }).children).toHaveLength(3)
    const table = within(complex).getByRole('table', { name: 'Models of complex by outcome' })
    expect(within(table).getByText('agy/claude-sonnet-4-6/thinking')).toBeInTheDocument()
    const ex = within(within(complex).getByRole('list', { name: 'Example tasks of complex' })).getByRole('listitem')
    for (const t of ['history', 'c=74', 'rejected', 'borderline', 'Refactor the session store to stream events']) expect(within(ex).getByText(t)).toBeInTheDocument()

    const simple = panel.querySelector('[data-insight-level="simple"]') as HTMLElement
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
    const puts: LevelsPut[] = []
    server.use(
      http.put('/api/v1/levels', async ({ request }) => {
        const b = (await request.json()) as LevelsPut
        puts.push(b)
        return HttpResponse.json({ ...fx.levels, ...b, restart_required: true })
      }),
    )
    renderWithProviders(<RoutingPage />, { route: '/routing' })
    const panel = await insightsRegion()
    const s = fx.insights.suggestions[0]
    expect(await within(panel).findByText('promote_model')).toBeInTheDocument()
    expect(within(panel).getByText(`${s.diff[0].level}.${s.diff[0].field}: ${s.diff[0].before} -> ${s.diff[0].after}`)).toBeInTheDocument()

    await userEvent.click(within(panel).getByRole('button', { name: `Apply suggestion ${s.id}` }))
    expect(puts).toHaveLength(0)
    const dialog = await screen.findByRole('alertdialog')
    await waitFor(() => expect(within(dialog).getByRole('button', { name: 'Apply suggestion' })).toBeEnabled())
    await userEvent.click(within(dialog).getByRole('button', { name: 'Apply suggestion' }))
    await waitFor(() => expect(puts).toHaveLength(1))
    expect(puts[0].levels).toEqual(s.levels)
    expect(puts[0].routing_instruction).toBe(fx.levels.routing_instruction)
    expect(await within(panel).findByRole('status')).toHaveTextContent(/Applied suggestion .*Restart/)
  })

  it('shows the API error when applying fails', async () => {
    server.use(http.put('/api/v1/levels', () => HttpResponse.json({ error: { code: -32602, message: 'invalid levels' } }, { status: 400 })))
    renderWithProviders(<RoutingPage />, { route: '/routing' })
    const panel = await insightsRegion()
    await userEvent.click(await within(panel).findByRole('button', { name: /Apply suggestion/ }))
    const dialog = await screen.findByRole('alertdialog')
    await waitFor(() => expect(within(dialog).getByRole('button', { name: 'Apply suggestion' })).toBeEnabled())
    await userEvent.click(within(dialog).getByRole('button', { name: 'Apply suggestion' }))
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
