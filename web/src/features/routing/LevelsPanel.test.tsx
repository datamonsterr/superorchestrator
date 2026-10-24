import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { describe, expect, it } from 'vitest'
import { server } from '@/test/server'
import { renderWithProviders } from '@/test/render'
import * as fx from '@/api/msw/fixtures'
import type { ChoicesView, Level, LevelsView } from '@/api/types'
import { RoutingPage } from './RoutingPage'

const SAVED: Level[] = [
  { name: 'low', min: 0, max: 40, tier: 'fast', choices: [{ id: 'agy-pro' }] },
  {
    name: 'high',
    min: 41,
    max: 100,
    choices: [
      { id: 'codex-high', priority: 1 },
      { id: 'gone', priority: 2 },
    ],
  },
]

/** A stateful stand-in for the levels endpoint; returns the PUT bodies seen. */
function mockLevels(initial: Level[] | null = SAVED) {
  let saved = initial
  const puts: Level[][] = []
  let resets = 0
  const view = (): LevelsView => (saved ? { ...fx.levelsViewOf(saved), restart_required: false } : fx.levels)
  server.use(
    http.get('/api/v1/levels', () => HttpResponse.json(view())),
    http.put('/api/v1/levels', async ({ request }) => {
      saved = ((await request.json()) as { levels: Level[] }).levels
      puts.push(saved)
      return HttpResponse.json(view())
    }),
    http.delete('/api/v1/levels', () => {
      saved = null
      resets++
      return HttpResponse.json(view())
    }),
  )
  return { puts, resets: () => resets }
}

const section = async (name: string) => within(await screen.findByRole('region', { name }))
const open = () => renderWithProviders(<RoutingPage />, { route: '/?tab=levels' })

describe('Levels tab', () => {
  it('lists levels with their models and states', async () => {
    mockLevels()
    open()
    const high = await section('high')
    expect(high.getByText('41–100')).toBeInTheDocument()
    const row = high.getByRole('row', { name: /codex-high/ })
    expect(within(row).getByText('gpt-5')).toBeInTheDocument()
    expect(within(row).getByText('high', { exact: true })).toBeInTheDocument()
    expect(within(row).getByText('on')).toBeInTheDocument()
    expect(within(high.getByRole('row', { name: /gone/ })).getByText('unknown')).toBeInTheDocument()
    const low = await section('low')
    expect(low.getByText('tier fast')).toBeInTheDocument()
    expect(screen.queryByText('Using the default levels')).not.toBeInTheDocument()
  })

  it('says when the default levels are in use, with the tier defaults', async () => {
    mockLevels(null)
    open()
    expect(await screen.findByText('Using the default levels')).toBeInTheDocument()
    expect(screen.getByText('low 0–37: fast')).toBeInTheDocument()
    expect(screen.getByText('high 63–100: high')).toBeInTheDocument()
  })

  it('shows the restart banner', async () => {
    server.use(http.get('/api/v1/levels', () => HttpResponse.json({ ...fx.levelsViewOf(SAVED), restart_required: true })))
    open()
    expect(await screen.findByText('Restart required')).toBeInTheDocument()
  })

  it('adds a level', async () => {
    const m = mockLevels()
    open()
    await userEvent.click(await screen.findByRole('button', { name: 'Add level' }))
    const dialog = await screen.findByRole('dialog', { name: 'Add level' })
    await userEvent.click(within(dialog).getByRole('button', { name: 'Save level' }))
    expect(await within(dialog).findAllByRole('alert')).not.toHaveLength(0)
    expect(m.puts).toHaveLength(0)
    await userEvent.type(within(dialog).getByLabelText('Name'), 'mid')
    await userEvent.clear(within(dialog).getByLabelText('Minimum'))
    await userEvent.type(within(dialog).getByLabelText('Minimum'), '41')
    await userEvent.click(within(dialog).getByRole('combobox', { name: /Tier/ }))
    await userEvent.click(await screen.findByRole('option', { name: 'mid' }))
    await userEvent.click(within(dialog).getByRole('button', { name: 'Save level' }))
    await waitFor(() => expect(m.puts).toHaveLength(1))
    expect(m.puts[0].at(-1)).toEqual({ name: 'mid', min: 41, max: 100, tier: 'mid', choices: [] })
    expect(m.puts[0]).toHaveLength(3)
    expect(await screen.findByRole('region', { name: 'mid' })).toBeInTheDocument()
  })

  it('edits a range', async () => {
    const m = mockLevels()
    open()
    await userEvent.click(await screen.findByRole('button', { name: 'Edit low' }))
    const dialog = await screen.findByRole('dialog', { name: 'Edit low' })
    await userEvent.clear(within(dialog).getByLabelText('Maximum'))
    await userEvent.type(within(dialog).getByLabelText('Maximum'), '45')
    await userEvent.click(within(dialog).getByRole('button', { name: 'Save level' }))
    await waitFor(() => expect(m.puts).toHaveLength(1))
    expect(m.puts[0][0]).toEqual({ name: 'low', min: 0, max: 45, tier: 'fast', choices: [{ id: 'agy-pro' }] })
    expect(m.puts[0][1].name).toBe('high')
    expect(await (await section('low')).findByText('0–45')).toBeInTheDocument()
  })

  it('deletes a level after confirming', async () => {
    const m = mockLevels()
    open()
    await userEvent.click(await screen.findByRole('button', { name: 'Delete low' }))
    await userEvent.click(await screen.findByRole('button', { name: 'Delete level' }))
    await waitFor(() => expect(m.puts).toHaveLength(1))
    expect(m.puts[0].map((l) => l.name)).toEqual(['high'])
  })

  it('adds an existing choice to a level with a priority', async () => {
    const m = mockLevels()
    open()
    await userEvent.click(await screen.findByRole('button', { name: 'Add model to low' }))
    const dialog = await screen.findByRole('dialog', { name: 'Add model to low' })
    await userEvent.click(within(dialog).getByRole('combobox', { name: 'Choice' }))
    expect(screen.queryByRole('option', { name: /agy-pro/ })).not.toBeInTheDocument()
    await userEvent.click(await screen.findByRole('option', { name: 'codex-high — codex / gpt-5 / high' }))
    await userEvent.type(within(dialog).getByLabelText(/Priority/), '3')
    await userEvent.click(within(dialog).getByRole('button', { name: 'Add model' }))
    await waitFor(() => expect(m.puts).toHaveLength(1))
    expect(m.puts[0][0].choices).toEqual([{ id: 'agy-pro' }, { id: 'codex-high', priority: 3 }])
    expect((await section('low')).getByRole('row', { name: /codex-high/ })).toBeInTheDocument()
  })

  it('creates a new choice, then lists it in the level', async () => {
    const m = mockLevels()
    const choicePuts: ChoicesView[] = []
    server.use(
      http.put('/api/v1/choices', async ({ request }) => {
        const b = (await request.json()) as ChoicesView
        choicePuts.push(b)
        return HttpResponse.json(b)
      }),
    )
    open()
    await userEvent.click(await screen.findByRole('button', { name: 'New model for low' }))
    const dialog = await screen.findByRole('dialog', { name: 'Add choice' })
    await userEvent.type(within(dialog).getByLabelText('Id'), 'codex-low')
    await userEvent.type(within(dialog).getByLabelText('Model'), 'gpt-5-mini')
    await userEvent.click(within(dialog).getByRole('button', { name: 'Save choice' }))
    await waitFor(() => expect(m.puts).toHaveLength(1))
    expect(choicePuts[0].choices.at(-1)?.id).toBe('codex-low')
    expect(m.puts[0][0].choices).toEqual([{ id: 'agy-pro' }, { id: 'codex-low' }])
  })

  it('removes a model from a level', async () => {
    const m = mockLevels()
    open()
    await userEvent.click(await screen.findByRole('button', { name: 'Remove codex-high from high' }))
    await waitFor(() => expect(m.puts).toHaveLength(1))
    expect(m.puts[0][1].choices).toEqual([{ id: 'gone', priority: 2 }])
    await waitFor(() => expect(screen.queryByRole('row', { name: /codex-high/ })).not.toBeInTheDocument())
  })

  it('changes priority by moving a model, renumbering 1..n', async () => {
    const m = mockLevels()
    open()
    await userEvent.click(await screen.findByRole('button', { name: 'Move gone up in high' }))
    await waitFor(() => expect(m.puts).toHaveLength(1))
    expect(m.puts[0][1].choices).toEqual([
      { id: 'gone', priority: 1 },
      { id: 'codex-high', priority: 2 },
    ])
    expect(screen.getByRole('button', { name: 'Move gone up in high' })).toBeDisabled()
  })

  it('resets to defaults after confirming', async () => {
    const m = mockLevels()
    open()
    await userEvent.click(await screen.findByRole('button', { name: 'Reset to defaults' }))
    await userEvent.click(await screen.findByRole('button', { name: 'Reset levels' }))
    await waitFor(() => expect(m.resets()).toBe(1))
    expect(await screen.findByText('Using the default levels')).toBeInTheDocument()
  })

  it('shows the server message for a rejected list, inside the dialog', async () => {
    mockLevels()
    server.use(http.put('/api/v1/levels', () => HttpResponse.json({ error: { code: -32602, message: 'levels "low" and "high" overlap' } }, { status: 422 })))
    open()
    await userEvent.click(await screen.findByRole('button', { name: 'Edit low' }))
    const dialog = await screen.findByRole('dialog', { name: 'Edit low' })
    await userEvent.click(within(dialog).getByRole('button', { name: 'Save level' }))
    expect(await within(dialog).findByText('levels "low" and "high" overlap')).toBeInTheDocument()
  })

  it('shows the server message for a rejected row action above the list', async () => {
    mockLevels()
    server.use(http.put('/api/v1/levels', () => HttpResponse.json({ error: { code: -32602, message: 'level "high" needs choices or a tier' } }, { status: 422 })))
    open()
    await userEvent.click(await screen.findByRole('button', { name: 'Remove codex-high from high' }))
    expect(await screen.findByText('level "high" needs choices or a tier')).toBeInTheDocument()
  })
})
