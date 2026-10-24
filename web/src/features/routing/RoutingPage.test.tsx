import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { describe, expect, it } from 'vitest'
import { server } from '@/test/server'
import { renderWithProviders } from '@/test/render'
import * as fx from '@/api/msw/fixtures'
import type { LevelsPut, RouteRecord } from '@/api/types'
import { RoutingPage } from './RoutingPage'

const level = (name: string) => screen.findByRole('region', { name: `Level ${name}` })
const refsOf = (put: LevelsPut, name: string) => put.levels.find((l) => l.name === name)?.models

function capturePut() {
  const puts: LevelsPut[] = []
  server.use(
    http.put('/api/v1/levels', async ({ request }) => {
      const b = (await request.json()) as LevelsPut
      puts.push(b)
      return HttpResponse.json({ ...fx.levels, ...b })
    }),
  )
  return puts
}

describe('Levels tab', () => {
  it('lists the levels in config order with their models, and a models panel with a refresh control', async () => {
    let refreshed = 0
    server.use(http.post('/api/v1/models/refresh', () => (refreshed++, HttpResponse.json(fx.models))))
    renderWithProviders(<RoutingPage />, { route: '/routing' })
    const complex = await level('complex')
    expect(within(complex).getByText('63–87')).toBeInTheDocument()
    const refs = within(within(complex).getByRole('list', { name: 'Models of complex' })).getAllByRole('listitem')
    expect(refs.map((r) => r.querySelector('span.font-mono')?.textContent)).toEqual(['agy/claude-sonnet-4-6/thinking', 'opencode/opencode/muse-spark-1.3', 'codex/gpt-6.1-sol/high'])
    expect(within(refs[2]).getByText('Codex')).toBeInTheDocument()
    expect(screen.getAllByRole('region', { name: /^Level / }).map((r) => r.getAttribute('aria-label'))).toEqual(['Level trivial', 'Level simple', 'Level moderate', 'Level complex', 'Level expert'])

    await userEvent.click(await screen.findByRole('button', { name: /Models \(/ }))
    expect(await screen.findByRole('region', { name: 'codex models' })).toBeInTheDocument()
    await userEvent.click(screen.getByRole('button', { name: 'Refresh models' }))
    await waitFor(() => expect(refreshed).toBe(1))
  })

  it('shows the empty state', async () => {
    server.use(http.get('/api/v1/levels', () => HttpResponse.json({ levels: [], routing_instruction: '', restart_required: false })))
    renderWithProviders(<RoutingPage />, { route: '/routing' })
    expect(await screen.findByText('No levels yet')).toBeInTheDocument()
    expect(screen.getAllByRole('button', { name: 'Add level' })).toHaveLength(2) // page header and empty state
  })

  it('adds a model through the dialog with a live ref preview, appending it to the level', async () => {
    const puts = capturePut()
    renderWithProviders(<RoutingPage />, { route: '/routing' })
    await userEvent.click(within(await level('simple')).getByRole('button', { name: 'Add model' }))
    const dialog = await screen.findByRole('dialog', { name: 'Add model to simple' })
    await userEvent.click(within(dialog).getByRole('button', { name: 'Save model' }))
    expect(await within(dialog).findByText('Enter a model')).toBeInTheDocument()
    expect(puts).toHaveLength(0)

    await userEvent.type(within(dialog).getByLabelText('Model'), 'gpt-6.1-sol')
    await userEvent.type(within(dialog).getByLabelText('Effort (optional)'), 'low')
    expect(within(dialog).getByLabelText('Resulting ref')).toHaveTextContent('codex/gpt-6.1-sol/low')
    await userEvent.click(within(dialog).getByRole('button', { name: 'Save model' }))
    await waitFor(() => expect(puts).toHaveLength(1))
    expect(refsOf(puts[0], 'simple')).toEqual([...fx.levels.levels[1].models, 'codex/gpt-6.1-sol/low'])
    expect(refsOf(puts[0], 'trivial')).toEqual(fx.levels.levels[0].models)
    expect(puts[0].routing_instruction).toBe(fx.levels.routing_instruction)
    await waitFor(() => expect(screen.queryByRole('dialog', { name: 'Add model to simple' })).not.toBeInTheDocument())
  })

  it('validates the model: opencode needs provider/model and a level cannot list a ref twice', async () => {
    const puts = capturePut()
    renderWithProviders(<RoutingPage />, { route: '/routing' })
    await userEvent.click(within(await level('simple')).getByRole('button', { name: 'Add model' }))
    const dialog = await screen.findByRole('dialog', { name: 'Add model to simple' })
    await userEvent.click(within(dialog).getByRole('combobox', { name: 'Harness' }))
    await userEvent.click(await screen.findByRole('option', { name: 'opencode' }))
    await userEvent.type(within(dialog).getByLabelText('Model'), 'muse-spark-1.3')
    await userEvent.click(within(dialog).getByRole('button', { name: 'Save model' }))
    expect(await within(dialog).findByText(/provider\/model/)).toBeInTheDocument()

    await userEvent.clear(within(dialog).getByLabelText('Model'))
    await userEvent.type(within(dialog).getByLabelText('Model'), 'opencode/deepseek-v4.1-flash-free')
    await userEvent.click(within(dialog).getByRole('button', { name: 'Save model' }))
    expect(await within(dialog).findByText('This level already lists this model')).toBeInTheDocument()
    expect(puts).toHaveLength(0)
  })

  it('moves a model up', async () => {
    const puts = capturePut()
    renderWithProviders(<RoutingPage />, { route: '/routing' })
    const complex = await level('complex')
    expect(within(complex).getByRole('button', { name: 'Move agy/claude-sonnet-4-6/thinking up' })).toBeDisabled()
    await userEvent.click(within(complex).getByRole('button', { name: 'Move codex/gpt-6.1-sol/high up' }))
    await waitFor(() => expect(puts).toHaveLength(1))
    expect(refsOf(puts[0], 'complex')).toEqual(['agy/claude-sonnet-4-6/thinking', 'codex/gpt-6.1-sol/high', 'opencode/opencode/muse-spark-1.3'])
  })

  it('removes a model', async () => {
    const puts = capturePut()
    renderWithProviders(<RoutingPage />, { route: '/routing' })
    await userEvent.click(within(await level('expert')).getByRole('button', { name: 'Remove codex/gpt-6.1-sol/high' }))
    await waitFor(() => expect(puts).toHaveLength(1))
    expect(refsOf(puts[0], 'expert')).toEqual(['opencode/opencode/muse-spark-1.3', 'agy/claude-sonnet-4-6/thinking'])
    expect(refsOf(puts[0], 'complex')).toEqual(fx.levels.levels[3].models)
  })

  it('adds a level, validating the range first', async () => {
    const puts = capturePut()
    renderWithProviders(<RoutingPage />, { route: '/routing' })
    await userEvent.click(await screen.findByRole('button', { name: /Add level/ }))
    const dialog = await screen.findByRole('dialog', { name: 'Add level' })
    await userEvent.type(within(dialog).getByLabelText('Name'), 'overlap')
    await userEvent.clear(within(dialog).getByLabelText('From'))
    await userEvent.type(within(dialog).getByLabelText('From'), '50')
    await userEvent.click(within(dialog).getByRole('button', { name: 'Save level' }))
    expect(await within(dialog).findByText('Overlaps moderate (38–62)')).toBeInTheDocument()
    expect(puts).toHaveLength(0)

    await userEvent.clear(within(dialog).getByLabelText('Name'))
    await userEvent.click(within(dialog).getByRole('button', { name: 'Save level' }))
    expect(await within(dialog).findByText('Enter a name')).toBeInTheDocument()
  })

  it('saves a new level with a tier when there is room for it', async () => {
    const puts = capturePut()
    server.use(
      http.get('/api/v1/levels', () => HttpResponse.json({ ...fx.levels, levels: fx.levels.levels.slice(0, 2) })),
    )
    renderWithProviders(<RoutingPage />, { route: '/routing' })
    await userEvent.click(await screen.findByRole('button', { name: /Add level/ }))
    const dialog = await screen.findByRole('dialog', { name: 'Add level' })
    expect(within(dialog).getByLabelText('From')).toHaveValue(38)
    await userEvent.type(within(dialog).getByLabelText('Name'), 'rest')
    await userEvent.click(within(dialog).getByRole('button', { name: 'Save level' }))
    expect(await within(dialog).findByText('A level without models needs a tier')).toBeInTheDocument()
    await userEvent.click(within(dialog).getByRole('combobox', { name: 'Tier (optional)' }))
    await userEvent.click(await screen.findByRole('option', { name: 'high' }))
    await userEvent.click(within(dialog).getByRole('button', { name: 'Save level' }))
    await waitFor(() => expect(puts).toHaveLength(1))
    expect(puts[0].levels.map((l) => l.name)).toEqual(['trivial', 'simple', 'rest'])
    expect(puts[0].levels[2]).toEqual({ name: 'rest', min: 38, max: 100, models: [], tier: 'high' })
  })

  it('edits a level and shows a server error for it in the dialog', async () => {
    server.use(
      http.put('/api/v1/levels', () => HttpResponse.json({ error: { code: -32602, message: 'router.levels `simple` (13..37) overlaps `trivial` (0..12)' } }, { status: 422 })),
    )
    renderWithProviders(<RoutingPage />, { route: '/routing' })
    await userEvent.click(within(await level('simple')).getByRole('button', { name: 'Edit level' }))
    const dialog = await screen.findByRole('dialog', { name: 'Edit level simple' })
    expect(within(dialog).getByLabelText('From')).toHaveValue(13)
    await userEvent.click(within(dialog).getByRole('button', { name: 'Save level' }))
    expect(await within(dialog).findByText(/overlaps/)).toBeInTheDocument()
    expect(screen.getByRole('dialog', { name: 'Edit level simple' })).toBeInTheDocument()
  })

  it('deletes a level after confirming', async () => {
    const puts = capturePut()
    renderWithProviders(<RoutingPage />, { route: '/routing' })
    await userEvent.click(within(await level('trivial')).getByRole('button', { name: 'Delete level' }))
    expect(puts).toHaveLength(0)
    await userEvent.click(within(await screen.findByRole('alertdialog')).getByRole('button', { name: 'Delete level' }))
    await waitFor(() => expect(puts).toHaveLength(1))
    expect(puts[0].levels.map((l) => l.name)).toEqual(['simple', 'moderate', 'complex', 'expert'])
  })

  it('does not offer to delete the last level', async () => {
    server.use(http.get('/api/v1/levels', () => HttpResponse.json({ ...fx.levels, levels: fx.levels.levels.slice(0, 1) })))
    renderWithProviders(<RoutingPage />, { route: '/routing' })
    const only = await level('trivial')
    expect(within(only).getByRole('button', { name: 'Delete level' })).toBeDisabled()
    expect(within(only).getByRole('button', { name: 'Delete level' })).toHaveAccessibleDescription('The last level cannot be deleted')
  })

  it('shows the error when a delete is rejected', async () => {
    server.use(http.put('/api/v1/levels', () => HttpResponse.json({ error: { code: -32602, message: 'the server refused' } }, { status: 422 })))
    renderWithProviders(<RoutingPage />, { route: '/routing' })
    await userEvent.click(within(await level('trivial')).getByRole('button', { name: 'Delete level' }))
    await userEvent.click(within(await screen.findByRole('alertdialog')).getByRole('button', { name: 'Delete level' }))
    expect(await screen.findByText('the server refused')).toBeInTheDocument()
    await waitFor(() => expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument())
  })

  it('shows a server error when removing a model is rejected', async () => {
    server.use(http.put('/api/v1/levels', () => HttpResponse.json({ error: { code: -32602, message: 'router.levels `trivial`: needs `models` or `tier`' } }, { status: 422 })))
    renderWithProviders(<RoutingPage />, { route: '/routing' })
    const trivial = await level('trivial')
    await userEvent.click(within(trivial).getByRole('button', { name: 'Remove agy/gemini-3.8-flash-low/low' }))
    expect(await screen.findByText(/needs `models` or `tier`/)).toBeInTheDocument()
  })
})

describe('Routing instruction tab', () => {
  it('edits, counts, reverts and saves', async () => {
    const puts = capturePut()
    renderWithProviders(<RoutingPage />, { route: '/routing?tab=instruction' })
    const box = await screen.findByRole('textbox', { name: 'Routing instruction' })
    const save = screen.getByRole('button', { name: 'Save' })
    expect(save).toBeDisabled()
    expect(box).toHaveValue(fx.levels.routing_instruction)
    await userEvent.type(box, ' Extra.')
    expect(save).toBeEnabled()
    expect(screen.getByText(`${(fx.levels.routing_instruction.length + 7).toLocaleString()} characters`)).toBeInTheDocument()
    await userEvent.click(screen.getByRole('button', { name: 'Revert' }))
    expect(box).toHaveValue(fx.levels.routing_instruction)
    await userEvent.type(box, ' Extra.')
    await userEvent.click(save)
    await waitFor(() => expect(puts).toHaveLength(1))
    expect(puts[0].routing_instruction).toBe(`${fx.levels.routing_instruction} Extra.`)
    expect(puts[0].levels).toHaveLength(5)
  })
})

describe('Routing instruction draft', () => {
  it('survives a background refetch of the levels', async () => {
    const { queryClient } = renderWithProviders(<RoutingPage />, { route: '/routing?tab=instruction' })
    const box = await screen.findByRole('textbox', { name: 'Routing instruction' })
    await userEvent.type(box, ' Draft.')
    server.use(http.get('/api/v1/levels', () => HttpResponse.json({ ...fx.levels, routing_instruction: 'changed elsewhere' })))
    await queryClient.invalidateQueries({ queryKey: ['levels'] })
    expect(screen.getByRole('textbox', { name: 'Routing instruction' })).toHaveValue(`${fx.levels.routing_instruction} Draft.`)
  })
})

describe('Preview tab', () => {
  it('requires a prompt, then shows the result with probability bars', async () => {
    let body: unknown
    server.use(
      http.post('/api/v1/route', async ({ request }) => {
        body = await request.json()
        return HttpResponse.json(fx.routeResult)
      }),
    )
    renderWithProviders(<RoutingPage />, { route: '/routing?tab=preview' })
    await userEvent.click(await screen.findByRole('button', { name: 'Run preview' }))
    expect(await screen.findByRole('alert')).toHaveTextContent('Enter a prompt')
    expect(body).toBeUndefined()
    expect(screen.queryByLabelText(/Role/)).not.toBeInTheDocument()
    expect(screen.getByText('A model name, or harness/model/effort to pin all three')).toBeInTheDocument()

    await userEvent.type(screen.getByLabelText('Prompt'), 'Refactor the API')
    await userEvent.type(screen.getByLabelText('Model'), 'gpt-6.1-sol')
    await userEvent.click(screen.getByRole('button', { name: 'Run preview' }))
    const card = await screen.findByLabelText('Route result')
    expect(within(card).getByText('source: jev')).toBeInTheDocument()
    expect(within(card).getByText('confidence 0.82')).toBeInTheDocument()
    expect(within(card).getByText('complexity 82.5')).toBeInTheDocument()
    expect(within(card).getByText('level high')).toBeInTheDocument()
    const bars = within(card).getByRole('list', { name: 'Complexity probabilities' })
    expect(within(bars).getAllByRole('listitem')).toHaveLength(5)
    expect(within(card).getByText('codex/gpt-6.1-sol/high')).toBeInTheDocument()
    expect(within(card).getByText(/strongest coding model/)).toBeInTheDocument()
    expect(body).toEqual({ prompt: 'Refactor the API', model: 'gpt-6.1-sol' })
  })

  it('shows a route error', async () => {
    server.use(http.post('/api/v1/route', () => HttpResponse.json({ error: { code: -32004, message: 'router offline' } }, { status: 503 })))
    renderWithProviders(<RoutingPage />, { route: '/routing?tab=preview' })
    await userEvent.type(await screen.findByLabelText('Prompt'), 'x')
    await userEvent.click(screen.getByRole('button', { name: 'Run preview' }))
    expect(await screen.findByText('router offline')).toBeInTheDocument()
  })
})

describe('Route log tab', () => {
  const records: RouteRecord[] = Array.from({ length: 25 }, (_, i) => ({
    ...fx.routeRecords[0],
    run_id: `run-${i}`,
    prompt_excerpt: `prompt ${i}`,
  }))

  it('paginates through the cursor', async () => {
    const limits: string[] = []
    server.use(
      http.get('/api/v1/routing/records', ({ request }) => {
        const u = new URL(request.url)
        limits.push(u.searchParams.get('limit') ?? '')
        const start = Number(u.searchParams.get('cursor') ?? 0)
        const items = records.slice(start, start + 10)
        return HttpResponse.json({ items, next_cursor: start + 10 < records.length ? String(start + 10) : null })
      }),
    )
    renderWithProviders(<RoutingPage />, { route: '/routing?tab=log' })
    expect(await screen.findByText('prompt 0')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Previous' })).toBeDisabled()
    await userEvent.click(screen.getByRole('button', { name: 'Next' }))
    expect(await screen.findByText('prompt 10')).toBeInTheDocument()
    expect(screen.getByText('Page 2')).toBeInTheDocument()
    await userEvent.click(screen.getByRole('button', { name: 'Next' }))
    expect(await screen.findByText('prompt 20')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Next' })).toBeDisabled()
    await userEvent.click(screen.getByRole('button', { name: 'Previous' }))
    expect(await screen.findByText('prompt 10')).toBeInTheDocument()
    expect(limits).toEqual(['10', '10', '10'])
  })

  it('shows an empty state', async () => {
    server.use(http.get('/api/v1/routing/records', () => HttpResponse.json({ items: [], next_cursor: null })))
    renderWithProviders(<RoutingPage />, { route: '/routing?tab=log' })
    expect(await screen.findByText('No routes yet')).toBeInTheDocument()
  })
})
