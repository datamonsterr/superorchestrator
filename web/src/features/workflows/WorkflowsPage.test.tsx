import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { Route, Routes } from 'react-router'
import { describe, expect, it } from 'vitest'
import { server } from '@/test/server'
import { renderWithProviders } from '@/test/render'
import { WorkflowRunPage } from './WorkflowRunPage'
import { WorkflowsPage } from './WorkflowsPage'

const err = (status: number, message = 'nope') => HttpResponse.json({ error: { code: status === 501 ? -32601 : -32003, message } }, { status })

describe('WorkflowsPage', () => {
  it('lists saved workflows', async () => {
    renderWithProviders(<WorkflowsPage />)
    const row = await screen.findByRole('row', { name: /ship-feature/ })
    expect(within(row).getByText('project')).toBeInTheDocument()
  })

  it('shows the empty and error states for saved workflows', async () => {
    server.use(http.get('/api/v1/workflows', () => HttpResponse.json([])))
    const { unmount } = renderWithProviders(<WorkflowsPage />)
    expect(await screen.findByText('No saved workflows')).toBeInTheDocument()
    unmount()
    server.use(http.get('/api/v1/workflows', () => err(500, 'boom')))
    renderWithProviders(<WorkflowsPage />)
    expect(await screen.findByRole('alert')).toHaveTextContent('boom')
  })

  it.each([501, 404])('shows "not available" for run history on %i', async (status) => {
    server.use(http.get('/api/v1/workflows/runs', () => err(status)))
    renderWithProviders(<WorkflowsPage />)
    await userEvent.click(await screen.findByRole('tab', { name: 'Runs' }))
    expect(await screen.findByText('Not available on this supervisor')).toBeInTheDocument()
  })

  it('lists runs', async () => {
    renderWithProviders(<WorkflowsPage />)
    await userEvent.click(await screen.findByRole('tab', { name: 'Runs' }))
    const row = await screen.findByRole('row', { name: /wf-1/ })
    expect(within(row).getByText('running')).toBeInTheDocument()
    expect(within(row).getByRole('link', { name: 'sess-1' })).toBeInTheDocument()
  })

  it('runs a workflow with JSON args and rejects invalid JSON', async () => {
    let body: unknown
    server.use(http.post('/api/v1/workflows/:name/run', async ({ request }) => ((body = await request.json()), HttpResponse.json({ id: 'wf-new' }))))
    renderWithProviders(<WorkflowsPage />)
    await userEvent.click(await screen.findByRole('button', { name: 'Run ship-feature' }))
    const dialog = await screen.findByRole('dialog')
    const args = within(dialog).getByRole('textbox', { name: 'Arguments' })
    await userEvent.type(args, '{{bad')
    expect(within(dialog).getByRole('alert')).toBeInTheDocument()
    expect(within(dialog).getByRole('button', { name: 'Run' })).toBeDisabled()
    await userEvent.clear(args)
    await userEvent.type(args, '{{"a":1}')
    await userEvent.click(within(dialog).getByRole('button', { name: 'Run' }))
    await waitFor(() => expect(body).toEqual({ args: { a: 1 } }))
  })
})

describe('WorkflowRunPage', () => {
  const renderRun = (id: string) =>
    renderWithProviders(
      <Routes>
        <Route path="/workflows/:id" element={<WorkflowRunPage />} />
      </Routes>,
      { route: `/workflows/${id}` },
    )

  it('shows steps with state and a link to the run', async () => {
    renderRun('wf-1')
    const row = await screen.findByRole('row', { name: /implement/ })
    expect(within(row).getByText('running')).toBeInTheDocument()
    expect(within(row).getByRole('link', { name: 'run-2' })).toHaveAttribute('href', '/sessions/sess-1/runs/run-2')
    expect(screen.getByRole('button', { name: /Run again/ })).toBeInTheDocument()
  })

  it('shows not-available and error states', async () => {
    server.use(http.get('/api/v1/workflows/runs/:id', () => err(501)))
    const { unmount } = renderRun('wf-1')
    expect(await screen.findByText('Not available on this supervisor')).toBeInTheDocument()
    unmount()
    server.use(http.get('/api/v1/workflows/runs/:id', () => err(500, 'kaput')))
    renderRun('wf-1')
    expect(await screen.findByRole('alert')).toHaveTextContent('kaput')
  })
})
