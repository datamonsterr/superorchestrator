import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { createMemoryRouter } from 'react-router'
import { AppShell } from '@/components/layout/AppShell'
import { RouteErrorBoundary } from '@/components/layout/RouteErrorBoundary'
import { describe, expect, it } from 'vitest'
import { server } from '@/test/server'
import { createTestQueryClient } from '@/test/render'
import App from './App'
import { createTestRouter } from './router'

const renderAt = (path: string) =>
  render(<App router={createTestRouter([path])} queryClient={createTestQueryClient()} />)

describe('routes', () => {
  it('redirects / to the dashboard inside the shell', async () => {
    renderAt('/')
    expect(await screen.findByRole('heading', { level: 1, name: 'Dashboard' })).toBeInTheDocument()
    expect(screen.getByRole('link', { name: 'Skip to content' })).toHaveAttribute('href', '#main')
    expect(screen.getByRole('navigation', { name: 'Main' })).toBeInTheDocument()
  })

  it.each([
    ['/sessions', 'Sessions'],
    ['/sessions/sess-1', 'Session timeline'],
    ['/sessions/sess-1/runs/run-1', 'Session timeline'],
    ['/accounts', 'Accounts'],
    ['/choices', 'Choices and routing'],
    ['/agents', 'Agents'],
    ['/agents/reviewer', 'Agent editor'],
    ['/workflows', 'Workflows'],
    ['/workflows/wf-1', 'Workflow run'],
    ['/settings', 'Settings'],
  ])('%s renders its lazy page', async (path, title) => {
    renderAt(path)
    expect(await screen.findByRole('heading', { level: 1, name: title })).toBeInTheDocument()
  })

  it('shows Not found for unknown paths', async () => {
    renderAt('/nope/nothing')
    expect(await screen.findByRole('heading', { level: 1, name: 'Page not found' })).toBeInTheDocument()
  })

  it('renders the token page outside the shell', async () => {
    renderAt('/token')
    expect(await screen.findByLabelText('Access token')).toBeInTheDocument()
    expect(screen.queryByRole('navigation', { name: 'Main' })).not.toBeInTheDocument()
  })

  it('marks the current nav item and navigates via the sidebar', async () => {
    const user = userEvent.setup()
    renderAt('/dashboard')
    await user.click(await screen.findByRole('link', { name: 'Accounts' }))
    expect(await screen.findByRole('heading', { level: 1, name: 'Accounts' })).toBeInTheDocument()
    expect(screen.getByRole('link', { name: 'Accounts' })).toHaveAttribute('aria-current', 'page')
  })
})

describe('401 handling', () => {
  it('sends the user to /token', async () => {
    server.use(http.get('*/api/v1/status', () => HttpResponse.json({ error: { code: -32000, message: 'auth' } }, { status: 401 })))
    const router = createTestRouter(['/dashboard'])
    render(<App router={router} queryClient={createTestQueryClient()} />)
    await screen.findByRole('heading', { name: 'Dashboard' })
    await screen.findByLabelText('Access token')
    expect(router.state.location.pathname).toBe('/token')
  })
})

describe('error boundary', () => {
  it('contains a crashing route and keeps the shell', async () => {
    const Boom = () => {
      throw new Error('kaboom')
    }
    const router = createMemoryRouter(
      [{ path: '/', element: <AppShell />, children: [{ path: 'boom', Component: Boom, ErrorBoundary: RouteErrorBoundary }] }],
      { initialEntries: ['/boom'] },
    )
    render(<App router={router} queryClient={createTestQueryClient()} />)
    expect(await screen.findByText('This page failed to load')).toBeInTheDocument()
    expect(screen.getByText('kaboom')).toBeInTheDocument()
    expect(screen.getByRole('navigation', { name: 'Main' })).toBeInTheDocument()
  })
})
