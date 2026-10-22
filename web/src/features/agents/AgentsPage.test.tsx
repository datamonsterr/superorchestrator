import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { Route, Routes } from 'react-router'
import { describe, expect, it } from 'vitest'
import { server } from '@/test/server'
import { renderWithProviders } from '@/test/render'
import * as fx from '@/api/msw/fixtures'
import { AgentEditor } from './AgentEditor'
import { AgentsPage } from './AgentsPage'

const renderEditor = (path: string) =>
  renderWithProviders(
    <Routes>
      <Route path="/agents" element={<AgentsPage />} />
      <Route path="/agents/:name" element={<AgentEditor />} />
    </Routes>,
    { route: path },
  )

describe('AgentsPage', () => {
  it('lists agents with source and harness', async () => {
    renderWithProviders(<AgentsPage />)
    const row = await screen.findByRole('row', { name: /imported/ })
    expect(within(row).getByText('Claude (read-only)')).toBeInTheDocument()
    expect(within(row).getByText('claude')).toBeInTheDocument()
    expect(within(await screen.findByRole('row', { name: /reviewer/ })).getByText('auto')).toBeInTheDocument()
  })

  it.each([501, 404])('shows "not available" when the agents endpoint returns %i', async (status) => {
    server.use(http.get('/api/v1/agents', () => HttpResponse.json({ error: { code: status === 501 ? 'not_implemented' : 'not_found', message: 'x' } }, { status })))
    renderWithProviders(<AgentsPage />)
    expect(await screen.findByText('Not available on this supervisor')).toBeInTheDocument()
  })

  it('shows empty and error states', async () => {
    server.use(http.get('/api/v1/agents', () => HttpResponse.json({ agents: [] })))
    const { unmount } = renderWithProviders(<AgentsPage />)
    expect(await screen.findByText('No custom agents yet')).toBeInTheDocument()
    unmount()
    server.use(http.get('/api/v1/agents', () => HttpResponse.json({ error: { code: -32004, message: 'down' } }, { status: 503 })))
    renderWithProviders(<AgentsPage />)
    expect(await screen.findByRole('alert')).toHaveTextContent('down')
  })
})

describe('AgentEditor', () => {
  it('saves an edit with the new source', async () => {
    let body: { source: string } | undefined
    server.use(
      http.put('/api/v1/agents/reviewer', async ({ request }) => {
        body = (await request.json()) as { source: string }
        return HttpResponse.json({ ok: true, warnings: ['codex ignores `tools`'] })
      }),
    )
    renderEditor('/agents/reviewer')
    const box = await screen.findByRole('textbox', { name: 'AGENT.md' })
    expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled()
    await userEvent.type(box, ' more')
    await userEvent.click(screen.getByRole('button', { name: 'Save' }))
    await waitFor(() => expect(body?.source).toMatch(/more$/))
    expect(await screen.findByText('codex ignores `tools`')).toBeInTheDocument()
  })

  it('shows 422 validation feedback from the server', async () => {
    server.use(
      http.put('/api/v1/agents/reviewer', () =>
        HttpResponse.json({ error: { code: -32602, message: 'frontmatter is invalid', data: { description: 'required' } } }, { status: 422 }),
      ),
    )
    renderEditor('/agents/reviewer')
    await userEvent.type(await screen.findByRole('textbox', { name: 'AGENT.md' }), 'x')
    await userEvent.click(screen.getByRole('button', { name: 'Save' }))
    const alert = await screen.findByText('The agent file was not saved')
    expect(alert.closest('[role=alert]')).toHaveTextContent('frontmatter is invalid')
    expect(alert.closest('[role=alert]')).toHaveTextContent('description: required')
  })

  it('warns and disables Save when the source is redacted', async () => {
    renderEditor('/agents/secretive')
    expect(await screen.findByText('Secret values are hidden')).toBeInTheDocument()
    await userEvent.type(screen.getByRole('textbox', { name: 'AGENT.md' }), 'x')
    expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled()
  })

  it('treats .claude imports as read-only', async () => {
    renderEditor('/agents/imported')
    expect(await screen.findByText('Read-only import')).toBeInTheDocument()
    expect(screen.getByRole('textbox', { name: 'AGENT.md' })).toHaveAttribute('readonly')
    expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled()
    expect(screen.getByRole('button', { name: /Delete/ })).toBeDisabled()
  })

  it('deletes after confirmation and returns to the list', async () => {
    let deleted = false
    server.use(http.delete('/api/v1/agents/reviewer', () => ((deleted = true), HttpResponse.json({ ok: true }))))
    renderEditor('/agents/reviewer')
    await userEvent.click(await screen.findByRole('button', { name: /Delete/ }))
    expect(deleted).toBe(false)
    await userEvent.click(within(await screen.findByRole('alertdialog')).getByRole('button', { name: 'Delete' }))
    await waitFor(() => expect(deleted).toBe(true))
    expect(await screen.findByRole('heading', { name: 'Agents' })).toBeInTheDocument()
  })

  it('creates a new agent from the template', async () => {
    let body: { name: string; source: string } | undefined
    server.use(
      http.post('/api/v1/agents', async ({ request }) => {
        body = (await request.json()) as typeof body
        return HttpResponse.json({ ok: true, warnings: [] }, { status: 201 })
      }),
    )
    renderEditor('/agents/new')
    expect(screen.getByRole('button', { name: 'Create agent' })).toBeDisabled()
    await userEvent.type(screen.getByRole('textbox', { name: /Name/ }), 'my-agent')
    await userEvent.click(screen.getByRole('button', { name: 'Create agent' }))
    await waitFor(() => expect(body).toMatchObject({ name: 'my-agent' }))
    expect(body?.source).toContain('description:')
  })

  it('shows an error when the agent cannot be loaded', async () => {
    renderEditor('/agents/missing')
    expect(await screen.findByRole('alert')).toHaveTextContent('no agent named')
  })

  it('shows "not available" for a missing agents route, but a normal error for an unknown agent', async () => {
    server.use(http.get('/api/v1/agents/:name', () => HttpResponse.json({ error: { code: 'not_implemented', message: 'x' } }, { status: 501 })))
    const { unmount } = renderEditor('/agents/reviewer')
    expect(await screen.findByText('Not available on this supervisor')).toBeInTheDocument()
    unmount()
    server.resetHandlers()
    renderEditor('/agents/missing')
    expect(await screen.findByRole('alert')).toHaveTextContent('no agent named')
    expect(screen.queryByText('Not available on this supervisor')).not.toBeInTheDocument()
  })

  it('makes the textarea read-only when the source is redacted', async () => {
    renderEditor('/agents/secretive')
    expect(await screen.findByRole('textbox', { name: 'AGENT.md' })).toHaveAttribute('readonly')
  })

  it('blocks saving a new agent that contains a [REDACTED] marker', async () => {
    renderEditor('/agents/new')
    await userEvent.type(screen.getByRole('textbox', { name: /Name/ }), 'my-agent')
    const box = screen.getByRole('textbox', { name: 'AGENT.md' })
    await userEvent.click(box)
    await userEvent.paste('key: [REDACTED]')
    expect(screen.getByRole('button', { name: 'Create agent' })).toBeDisabled()
    expect(screen.getByText(/Remove the/)).toBeInTheDocument()
  })

  it('tells the user when the file changed on disk under a dirty draft, and can reload', async () => {
    const { queryClient } = renderEditor('/agents/reviewer')
    const box = await screen.findByRole('textbox', { name: 'AGENT.md' })
    await userEvent.type(box, ' mine')
    const changed = { ...fx.agents[0], source: fx.agents[0].source + 'edited elsewhere\n' }
    server.use(http.get('/api/v1/agents/reviewer', () => HttpResponse.json(changed)))
    await queryClient.invalidateQueries({ queryKey: ['agent', 'reviewer'] })
    expect(await screen.findByText('Changed on disk')).toBeInTheDocument()
    await userEvent.click(screen.getByRole('button', { name: /Reload from disk/ }))
    expect(screen.queryByText('Changed on disk')).not.toBeInTheDocument()
    expect((screen.getByRole('textbox', { name: 'AGENT.md' }) as HTMLTextAreaElement).value).toContain('edited elsewhere')
  })

  it('silently follows the file when the draft is untouched', async () => {
    const { queryClient } = renderEditor('/agents/reviewer')
    await screen.findByRole('textbox', { name: 'AGENT.md' })
    server.use(http.get('/api/v1/agents/reviewer', () => HttpResponse.json({ ...fx.agents[0], source: 'new text' })))
    await queryClient.invalidateQueries({ queryKey: ['agent', 'reviewer'] })
    await waitFor(() => expect((screen.getByRole('textbox', { name: 'AGENT.md' }) as HTMLTextAreaElement).value).toBe('new text'))
    expect(screen.queryByText('Changed on disk')).not.toBeInTheDocument()
  })
})
