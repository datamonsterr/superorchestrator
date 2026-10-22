import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { describe, expect, it } from 'vitest'
import { server } from '@/test/server'
import { renderWithProviders } from '@/test/render'
import * as fx from '@/api/msw/fixtures'
import type { ChoicesView, RouteRecord } from '@/api/types'
import { RoutingPage } from './RoutingPage'

const choiceRow = (id: string) => screen.findByRole('row', { name: new RegExp(id) })
function capturePut() {
  const puts: ChoicesView[] = []
  server.use(
    http.put('/api/v1/choices', async ({ request }) => {
      const b = (await request.json()) as ChoicesView
      puts.push(b)
      return HttpResponse.json(b)
    }),
  )
  return puts
}

describe('Choices tab', () => {
  it('lists choices and models with a refresh control', async () => {
    let refreshed = 0
    server.use(http.post('/api/v1/models/refresh', () => (refreshed++, HttpResponse.json(fx.models))))
    renderWithProviders(<RoutingPage />, { route: '/choices' })
    const row = await choiceRow('codex-high')
    expect(within(row).getByText('gpt-5')).toBeInTheDocument()
    expect(within(row).getByRole('switch', { name: 'Enable codex-high' })).toBeChecked()
    expect(await screen.findByRole('button', { name: /Models \(/ })).toBeInTheDocument()
    await userEvent.click(screen.getByRole('button', { name: /Models \(/ }))
    expect(await screen.findByRole('region', { name: 'codex models' })).toBeInTheDocument()
    await userEvent.click(screen.getByRole('button', { name: 'Refresh models' }))
    await waitFor(() => expect(refreshed).toBe(1))
  })

  it('shows the empty state', async () => {
    server.use(http.get('/api/v1/choices', () => HttpResponse.json({ choices: [], routing_instruction: '' })))
    renderWithProviders(<RoutingPage />, { route: '/choices' })
    expect(await screen.findByText('No choices yet')).toBeInTheDocument()
  })

  it('toggles enabled with a full PUT', async () => {
    const puts = capturePut()
    renderWithProviders(<RoutingPage />, { route: '/choices' })
    await userEvent.click(await screen.findByRole('switch', { name: 'Enable agy-pro' }))
    await waitFor(() => expect(puts).toHaveLength(1))
    expect(puts[0].choices.find((c) => c.id === 'agy-pro')?.enabled).toBe(false)
    expect(puts[0].choices).toHaveLength(2)
    expect(puts[0].routing_instruction).toBe(fx.choices.routing_instruction)
  })

  it('adds a choice, validating first', async () => {
    const puts = capturePut()
    renderWithProviders(<RoutingPage />, { route: '/choices' })
    await userEvent.click(await screen.findByRole('button', { name: /Add choice/ }))
    const dialog = await screen.findByRole('dialog', { name: 'Add choice' })
    await userEvent.click(within(dialog).getByRole('button', { name: 'Save choice' }))
    expect(await within(dialog).findAllByRole('alert')).not.toHaveLength(0)
    expect(puts).toHaveLength(0)

    await userEvent.type(within(dialog).getByLabelText('Id'), 'codex-low')
    await userEvent.type(within(dialog).getByLabelText('Model'), 'gpt-5-mini')
    await userEvent.clear(within(dialog).getByLabelText('Roles'))
    await userEvent.type(within(dialog).getByLabelText('Roles'), 'worker, review')
    await userEvent.click(within(dialog).getByRole('button', { name: 'Save choice' }))
    await waitFor(() => expect(puts).toHaveLength(1))
    expect(puts[0].choices.at(-1)).toEqual({
      id: 'codex-low',
      harness: 'codex',
      model: 'gpt-5-mini',
      roles: ['worker', 'review'],
      description: '',
      enabled: true,
    })
    await waitFor(() => expect(screen.queryByRole('dialog', { name: 'Add choice' })).not.toBeInTheDocument())
  })

  it('edits a choice and shows server-side field errors from a 422', async () => {
    server.use(
      http.put('/api/v1/choices', () => HttpResponse.json({ error: { code: -32602, message: 'invalid', data: { model: 'model not offered by codex' } } }, { status: 422 })),
    )
    renderWithProviders(<RoutingPage />, { route: '/choices' })
    await choiceRow('codex-high')
    await userEvent.click(screen.getByRole('button', { name: 'Actions for codex-high' }))
    await userEvent.click(await screen.findByRole('menuitem', { name: 'Edit' }))
    const dialog = await screen.findByRole('dialog', { name: 'Edit codex-high' })
    expect(within(dialog).getByLabelText('Id')).toBeDisabled()
    await userEvent.clear(within(dialog).getByLabelText('Model'))
    await userEvent.type(within(dialog).getByLabelText('Model'), 'nope')
    await userEvent.click(within(dialog).getByRole('button', { name: 'Save choice' }))
    expect(await within(dialog).findByText('model not offered by codex')).toBeInTheDocument()
    expect(screen.getByRole('dialog', { name: 'Edit codex-high' })).toBeInTheDocument()
  })

  it('deletes a choice after confirming', async () => {
    const puts = capturePut()
    renderWithProviders(<RoutingPage />, { route: '/choices' })
    await choiceRow('agy-pro')
    await userEvent.click(screen.getByRole('button', { name: 'Actions for agy-pro' }))
    await userEvent.click(await screen.findByRole('menuitem', { name: 'Delete' }))
    await userEvent.click(await screen.findByRole('button', { name: 'Delete choice' }))
    await waitFor(() => expect(puts).toHaveLength(1))
    expect(puts[0].choices.map((c) => c.id)).toEqual(['codex-high'])
  })
})

describe('Routing instruction tab', () => {
  it('edits, counts, reverts and saves', async () => {
    const puts = capturePut()
    renderWithProviders(<RoutingPage />, { route: '/choices?tab=instruction' })
    const box = await screen.findByRole('textbox', { name: 'Routing instruction' })
    const save = screen.getByRole('button', { name: 'Save' })
    expect(save).toBeDisabled()
    expect(box).toHaveValue(fx.choices.routing_instruction)
    await userEvent.type(box, ' Extra.')
    expect(save).toBeEnabled()
    expect(screen.getByText(`${(fx.choices.routing_instruction.length + 7).toLocaleString()} characters`)).toBeInTheDocument()
    await userEvent.click(screen.getByRole('button', { name: 'Revert' }))
    expect(box).toHaveValue(fx.choices.routing_instruction)
    await userEvent.type(box, ' Extra.')
    await userEvent.click(save)
    await waitFor(() => expect(puts).toHaveLength(1))
    expect(puts[0].routing_instruction).toBe(`${fx.choices.routing_instruction} Extra.`)
    expect(puts[0].choices).toHaveLength(2)
  })
})

describe('Routing instruction draft', () => {
  it('survives a background refetch of the choices', async () => {
    const { queryClient } = renderWithProviders(<RoutingPage />, { route: '/choices?tab=instruction' })
    const box = await screen.findByRole('textbox', { name: 'Routing instruction' })
    await userEvent.type(box, ' Draft.')
    server.use(http.get('/api/v1/choices', () => HttpResponse.json({ ...fx.choices, routing_instruction: 'changed elsewhere' })))
    await queryClient.invalidateQueries({ queryKey: ['choices'] })
    expect(screen.getByRole('textbox', { name: 'Routing instruction' })).toHaveValue(`${fx.choices.routing_instruction} Draft.`)
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
    renderWithProviders(<RoutingPage />, { route: '/choices?tab=preview' })
    await userEvent.click(await screen.findByRole('button', { name: 'Run preview' }))
    expect(await screen.findByRole('alert')).toHaveTextContent('Enter a prompt')
    expect(body).toBeUndefined()

    await userEvent.type(screen.getByLabelText('Prompt'), 'Refactor the API')
    await userEvent.type(screen.getByLabelText('Model'), 'gpt-5')
    await userEvent.click(screen.getByRole('button', { name: 'Run preview' }))
    const card = await screen.findByLabelText('Route result')
    expect(within(card).getByText('source: jev')).toBeInTheDocument()
    expect(within(card).getByText('confidence 0.82')).toBeInTheDocument()
    const bars = within(card).getByRole('list', { name: 'Choice probabilities' })
    expect(within(bars).getAllByRole('listitem')).toHaveLength(3)
    expect(within(card).getByText(/strongest coding model/)).toBeInTheDocument()
    expect(body).toEqual({ prompt: 'Refactor the API', model: 'gpt-5' })
  })

  it('shows a route error', async () => {
    server.use(http.post('/api/v1/route', () => HttpResponse.json({ error: { code: -32004, message: 'router offline' } }, { status: 503 })))
    renderWithProviders(<RoutingPage />, { route: '/choices?tab=preview' })
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
    renderWithProviders(<RoutingPage />, { route: '/choices?tab=log' })
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
    renderWithProviders(<RoutingPage />, { route: '/choices?tab=log' })
    expect(await screen.findByText('No routes yet')).toBeInTheDocument()
  })
})
