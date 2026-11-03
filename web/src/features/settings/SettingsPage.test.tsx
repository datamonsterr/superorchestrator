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
      http.get('/api/v1/decider', () => HttpResponse.json({ ...fx.decider, key: { ...fx.decider.key, value: SECRET } })),
    )
    renderWithProviders(<SettingsPage />)
    await screen.findByLabelText('Global run cap')
    for (const tab of ['General', 'Notifications', 'Worktrees', 'Decider', 'UI']) {
      await openTab(tab)
      if (tab === 'Decider') {
        await screen.findByLabelText('Model')
        expect(screen.getByTestId('decider-key-current')).toHaveTextContent('a1b2')
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

describe('SettingsPage decider tab', () => {
  const openDecider = async () => {
    renderWithProviders(<SettingsPage />)
    await openTab('Decider')
    await screen.findByLabelText('Model')
  }
  const save = () => userEvent.click(screen.getByRole('button', { name: 'Save' }))
  const capture = () => {
    const seen: { body?: unknown } = {}
    server.use(
      http.patch('/api/v1/decider', async ({ request }) => {
        seen.body = await request.json()
        return HttpResponse.json(fx.decider)
      }),
    )
    return seen
  }

  it('renders the current values, in-effect models and key', async () => {
    await openDecider()
    expect(screen.getByLabelText('Endpoint')).toHaveValue('https://openrouter.ai/api/v1')
    expect(screen.getByLabelText('Model')).toHaveValue('openai/gpt-5-mini')
    expect(screen.getByLabelText('Fallback model')).toHaveValue('google/gemini-3-flash')
    expect(screen.getByLabelText('When the decider is unreachable')).toHaveValue('heuristic')
    expect(screen.getByLabelText('Timeout')).toHaveValue('20s')
    expect(screen.getByLabelText('Minimum confidence')).toHaveValue(0.45)
    expect(screen.getByTestId('decider-effective')).toHaveTextContent('openai/gpt-5-mini (config)')
    expect(screen.getByTestId('decider-key-current')).toHaveTextContent('From file /home/dev/.config/superorchestrator/decider.env: ...a1b2')
    expect(screen.getByLabelText('Model')).toHaveAttribute('list')
    expect(screen.getByLabelText('Model')).toHaveAccessibleDescription(/OpenRouter model id/)
    expect(document.querySelector('datalist option')).toHaveAttribute('value', 'openai/gpt-5-mini')
  })

  it('leaves the removed fields out of the other tabs', async () => {
    renderWithProviders(<SettingsPage />)
    await screen.findByLabelText('Global run cap')
    for (const tab of ['General', 'Notifications', 'Worktrees', 'UI']) {
      await openTab(tab)
      for (const l of ['Minimum confidence', 'Minimum margin', 'Key environment variable', 'Key file']) expect(screen.queryByLabelText(l)).not.toBeInTheDocument()
    }
    expect(screen.queryByRole('tab', { name: 'Routing' })).not.toBeInTheDocument()
  })

  it.each([
    ['Endpoint', 'http://example.com/v1', /https, or http only to a loopback host/],
    ['Model', 'a b', 'A model id cannot contain spaces'],
    ['Fallback model', 'x y', 'A model id cannot contain spaces'],
    ['Timeout', '121s', 'Timeout must be from 1s to 120s'],
    ['Timeout', '0s', 'Timeout must be from 1s to 120s'],
    ['Timeout', 'soon', 'Enter a duration such as 20s'],
    ['Minimum confidence', '1.5', 'At most 1'],
    ['Minimum margin', '-0.1', 'At least 0'],
  ])('validates %s = %s before sending', async (label, value, message) => {
    const seen = capture()
    await openDecider()
    const input = screen.getByLabelText(label)
    await userEvent.clear(input)
    await userEvent.type(input, value)
    await save()
    expect(await screen.findByText(message)).toBeInTheDocument()
    expect(seen.body).toBeUndefined()
  })

  it('accepts http to a loopback host', async () => {
    const seen = capture()
    await openDecider()
    const e = screen.getByLabelText('Endpoint')
    await userEvent.clear(e)
    await userEvent.type(e, 'http://localhost:8080/v1')
    await save()
    await waitFor(() => expect(seen.body).toEqual({ endpoint: 'http://localhost:8080/v1' }))
  })

  it('sends only the changed fields', async () => {
    const seen = capture()
    await openDecider()
    const model = screen.getByLabelText('Model')
    await userEvent.clear(model)
    await userEvent.type(model, 'anthropic/claude-haiku-5-5')
    await userEvent.selectOptions(screen.getByLabelText('When the decider is unreachable'), 'error')
    await save()
    await waitFor(() => expect(seen.body).toEqual({ model: 'anthropic/claude-haiku-5-5', on_unreachable: 'error' }))
  })

  it('clears the fallback model with an empty string', async () => {
    const seen = capture()
    await openDecider()
    await userEvent.clear(screen.getByLabelText('Fallback model'))
    await save()
    await waitFor(() => expect(seen.body).toEqual({ fallback_model: '' }))
  })

  it('sends an env var or file key when the source changes', async () => {
    const seen = capture()
    await openDecider()
    await userEvent.click(screen.getByRole('radio', { name: 'Environment variable' }))
    const name = screen.getByLabelText('Environment variable name')
    await userEvent.clear(name)
    await userEvent.type(name, 'MY_KEY')
    await save()
    await waitFor(() => expect(seen.body).toEqual({ key: { env: 'MY_KEY' } }))
  })

  it('pastes a key into a password input and clears it after saving', async () => {
    let body: unknown
    server.use(
      http.patch('/api/v1/decider', async ({ request }) => {
        body = await request.json()
        return HttpResponse.json({ ...fx.decider, key: { source: 'file', name: '/x/decider.env', masked: '...wxyz' } })
      }),
    )
    await openDecider()
    await userEvent.click(screen.getByRole('radio', { name: 'Paste key' }))
    const key = screen.getByLabelText('Key')
    expect(key).toHaveAttribute('type', 'password')
    expect(key).toHaveValue('')
    await userEvent.type(key, 'sk-or-v1-abcdwxyz')
    await save()
    await waitFor(() => expect(body).toEqual({ key: { paste: 'sk-or-v1-abcdwxyz' } }))
    await waitFor(() => expect(screen.getByTestId('decider-key-current')).toHaveTextContent('...wxyz'))
    await userEvent.click(screen.getByRole('radio', { name: 'Paste key' }))
    expect(screen.getByLabelText('Key')).toHaveValue('')
    expect(document.body.innerHTML).not.toContain('sk-or-v1-abcdwxyz')
  })

  it('says which variable to set when no key is found', async () => {
    server.use(http.get('/api/v1/decider', () => HttpResponse.json({ ...fx.decider, key: { source: 'unset', name: 'OPENROUTER_API_KEY' } })))
    await openDecider()
    expect(screen.getByTestId('decider-key-current')).toHaveTextContent('Set OPENROUTER_API_KEY')
  })

  it('shows an env override notice, with and without a value', async () => {
    server.use(
      http.get('/api/v1/decider', () =>
        HttpResponse.json({
          ...fx.decider,
          env_overrides: [
            { name: 'SUPERORCHESTRATOR_ROUTER_MODEL', field: 'model', value: 'x/forced' },
            { name: 'SUPERORCHESTRATOR_ROUTER_ENDPOINT', field: 'endpoint' },
          ],
          effective: { ...fx.decider.effective, primary: 'x/forced', primary_source: 'env' },
        }),
      ),
    )
    await openDecider()
    expect(screen.getByTestId('env-override-model')).toHaveTextContent('SUPERORCHESTRATOR_ROUTER_MODEL is set (x/forced) and wins over this setting until it is unset.')
    expect(screen.getByTestId('env-override-endpoint')).toHaveTextContent('SUPERORCHESTRATOR_ROUTER_ENDPOINT is set and wins')
    expect(screen.getByTestId('decider-effective')).toHaveTextContent('x/forced (env)')
  })

  it('shows the key file override notice in every key mode, without a value', async () => {
    server.use(
      http.get('/api/v1/decider', () =>
        HttpResponse.json({ ...fx.decider, env_overrides: [{ name: 'OPENROUTER_API_KEY', field: 'key_file', value: null }] }),
      ),
    )
    await openDecider()
    for (const mode of ['Environment variable', 'Key file', 'Paste key']) {
      await userEvent.click(screen.getByRole('radio', { name: mode }))
      expect(screen.getByTestId('env-override-key_file')).toHaveTextContent('OPENROUTER_API_KEY is set and wins over the key file until it is unset.')
    }
  })

  it('shows a server 400 on the named field', async () => {
    server.use(http.patch('/api/v1/decider', () => HttpResponse.json({ error: { code: -32602, message: 'model: unknown model id' } }, { status: 400 })))
    await openDecider()
    const model = screen.getByLabelText('Model')
    await userEvent.type(model, '-x')
    await save()
    expect(await screen.findByText('unknown model id')).toBeInTheDocument()
  })

  it('shows an unattributed server 400 as a form error', async () => {
    server.use(http.patch('/api/v1/decider', () => HttpResponse.json({ error: { code: -32602, message: 'config is locked' } }, { status: 400 })))
    await openDecider()
    await userEvent.type(screen.getByLabelText('Model'), '-x')
    await save()
    expect(await screen.findByText('config is locked')).toBeInTheDocument()
  })

  it('shows the restart notice when a restart is required', async () => {
    server.use(http.get('/api/v1/decider', () => HttpResponse.json({ ...fx.decider, restart_required: true })))
    await openDecider()
    expect(await screen.findByText('Restart required')).toBeInTheDocument()
  })

  it('tests the decider and shows the result', async () => {
    let body: unknown
    server.use(
      http.post('/api/v1/decider/test', async ({ request }) => {
        body = await request.json()
        return HttpResponse.json(fx.deciderTest)
      }),
    )
    await openDecider()
    await userEvent.type(screen.getByLabelText('Test prompt (optional)'), 'refactor the router')
    await userEvent.click(screen.getByRole('button', { name: 'Test decider' }))
    const r = await screen.findByRole('status', { name: 'Test result' })
    expect(body).toEqual({ prompt: 'refactor the router' })
    expect(r).toHaveTextContent('Complexity62.5')
    expect(r).toHaveTextContent('Levelhigh')
    expect(r).toHaveTextContent('Confidence0.81')
    expect(r).toHaveTextContent('Modelopenai/gpt-5-mini')
    expect(r).toHaveTextContent('412 ms')
  })

  it('shows a loading state while testing, then the error', async () => {
    let release: () => void = () => {}
    server.use(
      http.post('/api/v1/decider/test', async () => {
        await new Promise<void>((res) => (release = res))
        return HttpResponse.json({ ok: false, error: 'no key configured', latency_ms: 3 })
      }),
    )
    await openDecider()
    await userEvent.click(screen.getByRole('button', { name: 'Test decider' }))
    expect(await screen.findByRole('button', { name: 'Testing...' })).toBeDisabled()
    release()
    expect(await screen.findByText(/Test failed: no key configured/)).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Test decider' })).toBeEnabled()
  })
})
