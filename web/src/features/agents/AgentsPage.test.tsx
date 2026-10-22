import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { Route, Routes } from 'react-router'
import { describe, expect, it } from 'vitest'
import { server } from '@/test/server'
import { renderWithProviders } from '@/test/render'
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
})
