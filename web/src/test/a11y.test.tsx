import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { configureAxe } from 'vitest-axe'
import { describe, expect, it } from 'vitest'
import App from '@/App'
import { createTestRouter } from '@/router'
import { server } from '@/test/server'
import { createTestQueryClient } from '@/test/render'

// Each test renders the whole app and runs axe over it, so it gets a longer budget than the 5 s default.
const HEAVY = 20_000

// jsdom has no layout, so colour contrast cannot be computed here; src/lib/contrast.test.ts
// checks every state colour pair in light and dark instead.
const axe = configureAxe({ rules: { 'color-contrast': { enabled: false } } })

async function expectNoViolations() {
  const results = await axe(document.body)
  expect(results).toHaveNoViolations()
}
const renderAt = (path: string) => render(<App router={createTestRouter([path])} queryClient={createTestQueryClient()} />)
const heading = (name: string) => screen.findByRole('heading', { level: 1, name })
const apiErr = (status: number) => HttpResponse.json({ error: { code: -32004, message: 'down' } }, { status })

describe('axe: every page in its main state', () => {
  it.each<[string, string, () => Promise<unknown>]>([
    ['/dashboard', 'Dashboard', () => screen.findAllByRole('row', { name: /codex-acc1/ })],
    ['/sessions', 'Sessions', () => screen.findAllByRole('row', { name: /claude-code/ })],
    ['/accounts', 'Accounts', () => screen.findByRole('row', { name: /agy-acc1/ })],
    ['/routing', 'Levels and routing', () => screen.findByRole('region', { name: 'Level complex' })],
    ['/agents', 'Agents', () => screen.findByRole('row', { name: /reviewer/ })],
    ['/workflows', 'Workflows', () => screen.findByRole('row', { name: /ship-feature/ })],
    ['/settings', 'Settings', () => screen.findByLabelText('Global run cap')],
  ])('%s loaded', { timeout: HEAVY }, async (path, title, ready) => {
    renderAt(path)
    await heading(title)
    await ready()
    await expectNoViolations()
  })

  it('session timeline', { timeout: HEAVY }, async () => {
    renderAt('/sessions/sess-1')
    await screen.findByRole('group', { name: 'Session timeline' })
    await screen.findByRole('list', { name: 'Runs' })
    await expectNoViolations()
  })

  it('agent editor, loaded', { timeout: HEAVY }, async () => {
    renderAt('/agents/reviewer')
    await screen.findByRole('textbox', { name: 'AGENT.md' })
    await expectNoViolations()
  })

  it('agent editor, read-only and redacted warnings', { timeout: HEAVY }, async () => {
    const { unmount } = renderAt('/agents/imported')
    await screen.findByText('Read-only import')
    await expectNoViolations()
    unmount()
    renderAt('/agents/secretive')
    await screen.findByText('Secret values are hidden')
    await expectNoViolations()
  })

  it('new agent form', { timeout: HEAVY }, async () => {
    renderAt('/agents/new')
    await screen.findByRole('textbox', { name: /Name/ })
    await expectNoViolations()
  })

  it('workflows runs tab and run detail', { timeout: HEAVY }, async () => {
    const { unmount } = renderAt('/workflows')
    await userEvent.click(await screen.findByRole('tab', { name: 'Runs' }))
    await screen.findByRole('row', { name: /wf-1/ })
    await expectNoViolations()
    unmount()
    renderAt('/workflows/wf-1')
    await screen.findByRole('row', { name: /implement/ })
    await expectNoViolations()
  })

  it('settings tabs and the system tab', { timeout: HEAVY }, async () => {
    renderAt('/settings')
    await screen.findByLabelText('Global run cap')
    for (const tab of ['Notifications', 'Worktrees', 'Routing', 'UI']) {
      await userEvent.click(screen.getByRole('tab', { name: tab }))
      await expectNoViolations()
    }
    await userEvent.click(screen.getByRole('tab', { name: 'System' }))
    await screen.findByRole('row', { name: /agy binary/ })
    await expectNoViolations()
  })

  it('add account dialog', { timeout: HEAVY }, async () => {
    renderAt('/accounts')
    await userEvent.click(await screen.findByRole('button', { name: 'Add account' }))
    await screen.findByRole('dialog')
    await expectNoViolations()
  })

  it('token and not-found pages', { timeout: HEAVY }, async () => {
    const { unmount } = renderAt('/token')
    await screen.findByLabelText('Access token')
    await expectNoViolations()
    unmount()
    renderAt('/nope')
    await heading('Page not found')
    await expectNoViolations()
  })
})

describe('axe: empty and error states', () => {
  it.each([
    ['/agents', 'Agents', 'agents', () => HttpResponse.json({ agents: [] })],
    ['/workflows', 'Workflows', 'workflows', () => HttpResponse.json([])],
    ['/accounts', 'Accounts', 'accounts', () => HttpResponse.json([])],
  ])('%s empty', { timeout: HEAVY }, async (path, title, endpoint, body) => {
    server.use(http.get(`/api/v1/${endpoint}`, body))
    renderAt(path)
    await heading(title)
    await screen.findByText(/No |Add your first/)
    await expectNoViolations()
  })

  it.each([
    ['/agents', 'Agents', 'agents'],
    ['/settings', 'Settings', 'config'],
    ['/workflows', 'Workflows', 'workflows'],
  ])('%s error', { timeout: HEAVY }, async (path, title, endpoint) => {
    server.use(http.get(`/api/v1/${endpoint}`, () => apiErr(503)))
    renderAt(path)
    await heading(title)
    await screen.findByRole('alert')
    await expectNoViolations()
  })

  it('agents not available (501)', { timeout: HEAVY }, async () => {
    server.use(http.get('/api/v1/agents', () => HttpResponse.json({ error: { code: 'not_implemented', message: 'x' } }, { status: 501 })))
    renderAt('/agents')
    await screen.findByText('Not available on this supervisor')
    await expectNoViolations()
  })

  it('workflows not available (501)', { timeout: HEAVY }, async () => {
    server.use(http.get('/api/v1/workflows', () => apiErr(501)))
    renderAt('/workflows')
    await screen.findByText('Not available on this supervisor')
    await expectNoViolations()
  })
})
