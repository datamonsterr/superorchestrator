import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { toast } from 'sonner'
import { describe, expect, it, vi } from 'vitest'
import { paginate } from '@/api/msw/handlers'
import { runs } from '@/api/msw/fixtures'
import { qk } from '@/api/keys'
import type { RunViewPublic } from '@/api/types'
import { renderWithProviders } from '@/test/render'
import { server } from '@/test/server'
import { makeLargeSession } from '@/test/fixtures/large'
import { RunPanel } from './RunPanel'

const B = '/api/v1'
const withState = (id: string, over: Partial<RunViewPublic>) =>
  server.use(http.get(`${B}/runs/${id}`, () => HttpResponse.json({ ...runs.find((r) => r.run_id === id)!, ...over })))

function panel(runId = 'run-1', onClose = vi.fn()) {
  return { onClose, ...renderWithProviders(<RunPanel runId={runId} onClose={onClose} />) }
}
const tab = (name: string) => screen.findByRole('tab', { name })

describe('RunPanel', () => {
  it('shows the header with name, state, harness, account and model', async () => {
    panel('run-1')
    expect(await screen.findByRole('heading', { name: 'refactor-api' })).toBeInTheDocument()
    expect(screen.getByText('Running')).toBeInTheDocument()
    expect(screen.getByText('Codex')).toBeInTheDocument()
    expect(screen.getByText('codex-acc1')).toBeInTheDocument()
    expect(screen.getByText('gpt-5')).toBeInTheDocument()
    expect(screen.getByText('$0.42')).toBeInTheDocument()
  })

  it('has all seven tabs and switches between them', async () => {
    panel('run-1')
    for (const name of ['Brief', 'Route', 'Events', 'Transcript', 'Result', 'Worktree', 'Join']) expect(await tab(name)).toBeInTheDocument()
    expect(await screen.findByText('Refactor the API')).toBeInTheDocument() // Brief from run_queued
    await userEvent.click(screen.getByRole('tab', { name: 'Result' }))
    expect(await screen.findByText('No result yet.')).toBeInTheDocument()
    expect(screen.getByRole('tab', { name: 'Result' })).toHaveAttribute('aria-selected', 'true')
  })

  it('Route tab: bars sorted by probability, selected highlighted, rationale and alternatives', async () => {
    panel('run-1')
    await userEvent.click(await tab('Route'))
    const bars = await screen.findByRole('list', { name: 'Choice probabilities' })
    const rows = within(bars).getAllByRole('listitem')
    const pcts = rows.map((r) => Number(within(r).getByText(/%$/).textContent!.replace('%', '')))
    expect([...pcts].sort((a, b) => b - a)).toEqual(pcts)
    expect(rows.filter((r) => r.hasAttribute('data-selected'))).toHaveLength(1)
    expect(screen.getByText('Rationale')).toBeInTheDocument()
    expect(screen.getByText('Alternatives')).toBeInTheDocument()
  })

  it('Worktree tab shows path, branch and cwd with copy buttons', async () => {
    panel('run-1')
    await userEvent.click(await tab('Worktree'))
    expect(await screen.findByText('/work/wt/run-1')).toBeInTheDocument()
    expect(screen.getByText('so/run-1')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Copy path' })).toBeInTheDocument()
    expect(screen.getByText('No checks have run.')).toBeInTheDocument()
  })

  it('Result tab shows the error alert for a failed run', async () => {
    withState('run-1', { state: 'failed', error_kind: 'quota', result: { summary: 'partial' } })
    panel('run-1')
    await userEvent.click(await tab('Result'))
    expect(await screen.findByRole('alert')).toHaveTextContent('Run failed: quota')
    expect(screen.getAllByText('Result').length).toBeGreaterThan(1)
  })

  describe('Join tab', () => {
    it('builds the command from argv and cwd, copies it, and renders env names but no values', async () => {
      const writeText = vi.fn().mockResolvedValue(undefined)
      vi.stubGlobal('navigator', { ...navigator, clipboard: { writeText } })
      let posts = 0
      server.use(
        http.post(`${B}/runs/run-1/join`, () => {
          posts++
          return HttpResponse.json({ argv: ['codex', '--resume', 'abc 123', '--api-key', '***'], cwd: '/work/wt/run-1', env_names: ['CODEX_HOME'] })
        }),
      )
      panel('run-1')
      expect(posts).toBe(0) // not before the tab opens
      await userEvent.click(await tab('Join'))
      const cmd = "cd /work/wt/run-1 && codex --resume 'abc 123' --api-key '***'"
      expect(await screen.findByText(cmd)).toBeInTheDocument()
      expect(posts).toBe(1)
      expect(screen.getByText('CODEX_HOME')).toBeInTheDocument()
      expect(screen.queryByText(/CODEX_HOME=/)).toBeNull()
      expect(document.body.textContent).not.toMatch(/CODEX_HOME\s*=/)
      await userEvent.click(screen.getByRole('button', { name: 'Copy command' }))
      await waitFor(() => expect(writeText).toHaveBeenCalledWith(cmd))
    })

    it('shows an error with retry when the join call fails', async () => {
      server.use(http.post(`${B}/runs/run-1/join`, () => HttpResponse.json({ error: { code: -32000, message: 'no session' } }, { status: 500 })))
      panel('run-1')
      await userEvent.click(await tab('Join'))
      expect(await screen.findByText('Could not get the join command')).toBeInTheDocument()
    })
  })
})

describe('RunPanel actions', () => {
  it('running: send message and cancel enabled, no accept or reject', async () => {
    panel('run-1')
    await screen.findByRole('heading', { name: 'refactor-api' })
    expect(screen.getByRole('button', { name: 'Send message' })).toBeEnabled()
    expect(screen.getByRole('button', { name: 'Cancel run' })).toBeEnabled()
    expect(screen.queryByRole('button', { name: 'Accept' })).toBeNull()
    expect(screen.queryByRole('button', { name: 'Reject' })).toBeNull()
  })

  it('pending_review: shows accept and reject', async () => {
    panel('run-2')
    expect(await screen.findByRole('button', { name: 'Accept' })).toBeEnabled()
    expect(screen.getByRole('button', { name: 'Reject' })).toBeEnabled()
  })

  it('terminal: cancel disabled and the send button says "Send and resume"', async () => {
    withState('run-1', { state: 'succeeded' })
    panel('run-1')
    expect(await screen.findByRole('button', { name: 'Send and resume' })).toBeEnabled()
    expect(screen.getByRole('button', { name: 'Cancel run' })).toBeDisabled()
    expect(screen.queryByRole('button', { name: 'Accept' })).toBeNull()
  })

  it('sends a message with its kind, toasts, and invalidates the run', async () => {
    const success = vi.spyOn(toast, 'success').mockReturnValue('id')
    let body: unknown
    server.use(http.post(`${B}/runs/run-1/messages`, async ({ request }) => { body = await request.json(); return HttpResponse.json({ ok: true }) }))
    const { queryClient } = panel('run-1')
    await screen.findByRole('heading', { name: 'refactor-api' })
    const invalidate = vi.spyOn(queryClient, 'invalidateQueries')
    await userEvent.click(screen.getByRole('button', { name: 'Send message' }))
    const send = screen.getAllByRole('button', { name: 'Send' }).at(-1)!
    expect(send).toBeDisabled() // empty text
    await userEvent.type(screen.getByRole('textbox', { name: 'Message' }), 'please add tests')
    await userEvent.click(send)
    await waitFor(() => expect(body).toEqual({ text: 'please add tests', kind: 'message' }))
    await waitFor(() => expect(success).toHaveBeenCalledWith('Message sent'))
    expect(invalidate).toHaveBeenCalledWith({ queryKey: qk.run('run-1') })
    await waitFor(() => expect(screen.queryByRole('textbox', { name: 'Message' })).toBeNull())
  })

  it('cancel asks for confirmation, then posts, toasts and invalidates', async () => {
    const success = vi.spyOn(toast, 'success').mockReturnValue('id')
    let cancelled = 0
    server.use(http.post(`${B}/runs/run-1/cancel`, () => { cancelled++; return HttpResponse.json({ ok: true }) }))
    const { queryClient } = panel('run-1')
    await screen.findByRole('heading', { name: 'refactor-api' })
    const invalidate = vi.spyOn(queryClient, 'invalidateQueries')
    await userEvent.click(screen.getByRole('button', { name: 'Cancel run' }))
    const dialog = await screen.findByRole('alertdialog')
    expect(dialog).toHaveTextContent('its subtree')
    expect(cancelled).toBe(0)
    await userEvent.click(within(dialog).getByRole('button', { name: 'Cancel run' }))
    await waitFor(() => expect(cancelled).toBe(1))
    await waitFor(() => expect(success).toHaveBeenCalledWith('Run cancelled'))
    expect(invalidate).toHaveBeenCalledWith({ queryKey: qk.run('run-1') })
  })

  it('accept posts and toasts', async () => {
    const success = vi.spyOn(toast, 'success').mockReturnValue('id')
    let accepted = 0
    server.use(http.post(`${B}/runs/run-2/accept`, () => { accepted++; return HttpResponse.json({ ok: true }) }))
    panel('run-2')
    await userEvent.click(await screen.findByRole('button', { name: 'Accept' }))
    await waitFor(() => expect(accepted).toBe(1))
    await waitFor(() => expect(success).toHaveBeenCalledWith('Run accepted'))
  })

  it('reject requires feedback and sends it', async () => {
    let body: unknown
    server.use(http.post(`${B}/runs/run-2/reject`, async ({ request }) => { body = await request.json(); return HttpResponse.json({ ok: true }) }))
    panel('run-2')
    await userEvent.click(await screen.findByRole('button', { name: 'Reject' }))
    const dialog = await screen.findByRole('dialog', { name: 'Reject this run' })
    const confirm = within(dialog).getByRole('button', { name: 'Reject run' })
    expect(confirm).toBeDisabled()
    await userEvent.type(within(dialog).getByRole('textbox', { name: 'Feedback' }), '   ')
    expect(confirm).toBeDisabled()
    await userEvent.type(within(dialog).getByRole('textbox', { name: 'Feedback' }), 'tests are missing')
    expect(confirm).toBeEnabled()
    await userEvent.click(confirm)
    await waitFor(() => expect(body).toEqual({ feedback: 'tests are missing' }))
  })

  it('keeps the dialog open and does not toast success when the action fails', async () => {
    const success = vi.spyOn(toast, 'success').mockReturnValue('id')
    server.use(http.post(`${B}/runs/run-1/cancel`, () => HttpResponse.json({ error: { code: -32000, message: 'boom' } }, { status: 500 })))
    panel('run-1')
    await screen.findByRole('heading', { name: 'refactor-api' })
    await userEvent.click(screen.getByRole('button', { name: 'Cancel run' }))
    const dialog = await screen.findByRole('alertdialog')
    await userEvent.click(within(dialog).getByRole('button', { name: 'Cancel run' }))
    await waitFor(() => expect(within(dialog).getByRole('button', { name: 'Cancel run' })).toBeEnabled())
    expect(success).not.toHaveBeenCalled()
    expect(screen.getByRole('alertdialog')).toBeInTheDocument()
  })
})

/** jsdom has no layout: a 340 px scroller and 34 px rows, so the virtualizer shows about ten rows. */
function layout() {
  vi.spyOn(HTMLElement.prototype, 'clientHeight', 'get').mockReturnValue(340)
  vi.spyOn(HTMLElement.prototype, 'offsetHeight', 'get').mockReturnValue(34)
}

describe('RunPanel with 10 000 rows', () => {
  const large = makeLargeSession({ events: 10000, runs: 30, transcriptRows: 10000, seed: 1 })

  it('Events tab: virtualises 10 000 events, filters by group and expands a row', async () => {
    layout()
    server.use(http.get(`${B}/runs/run-1/events`, ({ request }) => HttpResponse.json(paginate(large.events, new URL(request.url), 1000))))
    panel('run-1')
    await userEvent.click(await tab('Events'))
    expect(await screen.findByText('10000 of 10000 events', {}, { timeout: 8000 })).toBeInTheDocument()
    const list = screen.getByRole('list', { name: 'Events' })
    const rendered = within(list).getAllByRole('listitem').length
    expect(rendered).toBeGreaterThan(5)
    expect(rendered).toBeLessThan(60)

    // Turn off every group but stalls.
    for (const name of ['Lifecycle', 'Activity', 'Messages']) await userEvent.click(screen.getByRole('button', { name }))
    const status = screen.getAllByRole('status').find((s) => /events$/.test(s.textContent ?? ''))!
    const stalls = large.events.filter((e) => ['stall_detected', 'nudge_sent', 'escalated'].includes(e.type)).length
    expect(status).toHaveTextContent(`${stalls} of 10000 events`)

    const first = within(list).getAllByRole('button')[0]
    await userEvent.click(first)
    expect(first).toHaveAttribute('aria-expanded', 'true')
    expect(within(list).getAllByText(/"/).length).toBeGreaterThan(0)
  }, 30000)

  it('Transcript tab: loads all pages, virtualises 10 000 lines and searches client-side', async () => {
    layout()
    panel('run-000')
    await userEvent.click(await tab('Transcript'))
    expect(await screen.findByText('10000 lines', {}, { timeout: 15000 })).toBeInTheDocument()
    const log = screen.getByRole('log', { name: 'Transcript' })
    const rendered = log.querySelectorAll('[data-index]').length
    expect(rendered).toBeGreaterThan(5)
    expect(rendered).toBeLessThan(60)
    await userEvent.type(screen.getByRole('searchbox', { name: 'Search transcript' }), 'line 777:')
    expect(await screen.findByText(/1 of 10000 lines match/)).toBeInTheDocument()
    expect(within(log).getByText(/assistant line 777:|user line 777:|system line 777:/)).toBeInTheDocument()
  }, 30000)

  it('Transcript tab: loads the fixture transcript for a small run', async () => {
    layout()
    panel('run-1')
    await userEvent.click(await tab('Transcript'))
    const log = await screen.findByRole('log', { name: 'Transcript' })
    expect(await screen.findByText(/lines$/)).toBeInTheDocument()
    expect(log.querySelectorAll('[data-index]').length).toBeGreaterThan(0)
  })
})
