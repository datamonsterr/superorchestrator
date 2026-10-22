import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { toast } from 'sonner'
import { describe, expect, it, vi } from 'vitest'
import { server } from '@/test/server'
import { renderWithProviders } from '@/test/render'
import * as fx from '@/api/msw/fixtures'
import { SettingsPage } from './SettingsPage'

vi.mock('sonner', () => ({ toast: Object.assign(vi.fn(), { success: vi.fn(), error: vi.fn() }), Toaster: () => null }))

const openTab = async (name: string) => userEvent.click(await screen.findByRole('tab', { name }))

describe('SettingsPage config', () => {
  it('saves only the changed field and shows the restart banner', async () => {
    let body: unknown
    server.use(
      http.get('/api/v1/config', () => HttpResponse.json({ ...fx.config, restart_required: body !== undefined })),
      http.patch('/api/v1/config', async ({ request }) => {
        body = await request.json()
        return HttpResponse.json({ ...fx.config, restart_required: true })
      }),
    )
    renderWithProviders(<SettingsPage />)
    const cap = await screen.findByLabelText('Global run cap')
    expect(cap).toHaveValue(8)
    expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled()
    await userEvent.clear(cap)
    await userEvent.type(cap, '4')
    await userEvent.click(screen.getByRole('button', { name: 'Save' }))
    await waitFor(() => expect(body).toEqual({ limits: { global_cap: 4 } }))
    expect(await screen.findByText('Restart required')).toBeInTheDocument()
  })

  it('validates before sending', async () => {
    let called = false
    server.use(http.patch('/api/v1/config', () => ((called = true), HttpResponse.json(fx.config))))
    renderWithProviders(<SettingsPage />)
    const cap = await screen.findByLabelText('Global run cap')
    await userEvent.clear(cap)
    await userEvent.type(cap, '0')
    await userEvent.click(screen.getByRole('button', { name: 'Save' }))
    expect(await screen.findByText('At least 1')).toBeInTheDocument()
    expect(called).toBe(false)
  })

  it('shows a server error on a failed save', async () => {
    server.use(http.patch('/api/v1/config', () => HttpResponse.json({ error: { code: -32602, message: 'bad port' } }, { status: 422 })))
    renderWithProviders(<SettingsPage />)
    const cap = await screen.findByLabelText('Global run cap')
    await userEvent.clear(cap)
    await userEvent.type(cap, '3')
    await userEvent.click(screen.getByRole('button', { name: 'Save' }))
    expect(await screen.findByText('bad port')).toBeInTheDocument()
  })

  it('shows loading and error states', async () => {
    server.use(http.get('/api/v1/config', () => HttpResponse.json({ error: { code: -32004, message: 'no config' } }, { status: 503 })))
    renderWithProviders(<SettingsPage />)
    expect(screen.getByRole('status', { name: 'Loading' })).toBeInTheDocument()
    expect(await screen.findByRole('alert')).toHaveTextContent('no config')
  })

  it('never renders a secret value, only the key source', async () => {
    const SECRET = 'sk-or-v1-TOPSECRET'
    server.use(
      http.get('/api/v1/config', () =>
        HttpResponse.json({
          ...fx.config,
          // Even if a server bug leaked values into the payload, nothing may print them.
          openrouter_key: { source: 'env', name: 'OPENROUTER_API_KEY', value: SECRET },
          ui_token: { source: 'file', name: null, value: SECRET },
          config: { ...fx.config.config, ui: { ...fx.config.config.ui, token: SECRET }, router: { jev: { key_env: 'OPENROUTER_API_KEY', key: SECRET } } },
        }),
      ),
    )
    renderWithProviders(<SettingsPage />)
    await screen.findByLabelText('Global run cap')
    for (const tab of ['General', 'Notifications', 'Worktrees', 'Routing', 'UI']) {
      await openTab(tab)
      if (tab === 'Routing') {
        const k = await screen.findByTestId('key-source-OpenRouter key')
        expect(k).toHaveTextContent('Environment variable')
        expect(k).toHaveTextContent('OPENROUTER_API_KEY')
        expect(k).toHaveTextContent('Key set: yes')
      }
      if (tab === 'UI') expect(await screen.findByTestId('key-source-UI token')).toHaveTextContent('File')
      expect(document.body.innerHTML).not.toContain(SECRET)
      for (const el of document.querySelectorAll('input,textarea')) expect((el as HTMLInputElement).value).not.toContain(SECRET)
    }
  })
})

describe('SettingsPage drafts and key file', () => {
  it('keeps the draft when the System tab is opened and closed', async () => {
    renderWithProviders(<SettingsPage />)
    const cap = await screen.findByLabelText('Global run cap')
    await userEvent.clear(cap)
    await userEvent.type(cap, '3')
    await openTab('System')
    await screen.findByRole('row', { name: /agy binary/ })
    expect(screen.queryByRole('button', { name: 'Save' })).not.toBeInTheDocument()
    await openTab('General')
    expect(await screen.findByLabelText('Global run cap')).toHaveValue(3)
    expect(screen.getByRole('button', { name: 'Save' })).toBeEnabled()
  })

  it('rejects clearing a saved key file with an inline error and sends nothing', async () => {
    let called = false
    server.use(
      http.get('/api/v1/config', () =>
        HttpResponse.json({ ...fx.config, config: { ...fx.config.config, router: { jev: { key_env: 'OPENROUTER_API_KEY', key_file: '/etc/or.env' } } } }),
      ),
      http.patch('/api/v1/config', () => ((called = true), HttpResponse.json(fx.config))),
    )
    renderWithProviders(<SettingsPage />)
    await screen.findByLabelText('Global run cap')
    await openTab('Routing')
    const file = await screen.findByLabelText('Key file')
    expect(file).toHaveValue('/etc/or.env')
    await userEvent.clear(file)
    await userEvent.click(screen.getByRole('button', { name: 'Save' }))
    expect(await screen.findByText(/cannot be cleared here/)).toBeInTheDocument()
    expect(called).toBe(false)
  })

  it('says "Nothing to save" and resets when the edit changes nothing', async () => {
    let called = false
    server.use(http.patch('/api/v1/config', () => ((called = true), HttpResponse.json(fx.config))))
    renderWithProviders(<SettingsPage />)
    await screen.findByLabelText('Global run cap')
    await openTab('Worktrees')
    const dir = await screen.findByLabelText('Worktree folder')
    await userEvent.type(dir, '   ')
    await userEvent.click(screen.getByRole('button', { name: 'Save' }))
    await waitFor(() => expect(toast).toHaveBeenCalledWith('Nothing to save'))
    expect(called).toBe(false)
    await waitFor(() => expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled())
    expect(screen.getByLabelText('Worktree folder')).toHaveValue('')
  })
})

describe('SettingsPage system tab', () => {
  it('lists doctor results and worktrees', async () => {
    renderWithProviders(<SettingsPage />)
    await openTab('System')
    const row = await screen.findByRole('row', { name: /agy binary/ })
    expect(within(row).getByText('Failed')).toBeInTheDocument()
    expect(await screen.findByRole('row', { name: /so\/old/ })).toHaveTextContent('Orphaned')
  })

  it('dry-runs the prune first and only deletes after confirmation', async () => {
    const calls: unknown[] = []
    server.use(
      http.post('/api/v1/worktrees/prune', async ({ request }) => {
        const b = (await request.json()) as { dry_run: boolean }
        calls.push(b)
        return HttpResponse.json({ removed: ['run-0'], skipped: b.dry_run ? [] : ['run-9'] })
      }),
    )
    renderWithProviders(<SettingsPage />)
    await openTab('System')
    await userEvent.click(await screen.findByRole('button', { name: 'Prune orphaned' }))
    const dialog = await screen.findByRole('alertdialog')
    expect(dialog).toHaveTextContent('run-0 (/work/wt/old)')
    expect(calls).toEqual([{ dry_run: true }])
    await userEvent.click(within(dialog).getByRole('button', { name: 'Prune' }))
    // The real prune is limited to the runs the user confirmed.
    await waitFor(() => expect(calls).toEqual([{ dry_run: true }, { dry_run: false, ids: ['run-0'] }]))
    await waitFor(() => expect(toast.success).toHaveBeenCalledWith('Pruned 1 run, skipped 1'))
  })

  it('does not ask for confirmation when nothing would be pruned', async () => {
    server.use(http.post('/api/v1/worktrees/prune', () => HttpResponse.json({ removed: [] })))
    renderWithProviders(<SettingsPage />)
    await openTab('System')
    await userEvent.click(await screen.findByRole('button', { name: 'Prune orphaned' }))
    await waitFor(() => expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument())
  })

  it('shows not-available when doctor is missing', async () => {
    server.use(http.get('/api/v1/doctor', () => HttpResponse.json({ error: { code: -32601, message: 'x' } }, { status: 501 })))
    renderWithProviders(<SettingsPage />)
    await openTab('System')
    expect(await screen.findByText('Not available on this supervisor')).toBeInTheDocument()
  })
})
