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

describe('Decider tab', () => {
  const openDecider = async () => {
    const r = renderWithProviders(<SettingsPage />)
    await openTab('Decider')
    await screen.findByLabelText('Model')
    return r
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

  it('pastes a key into a password input, clears it after saving and leaves no secret in the cache', async () => {
    const SECRET = 'sk-or-v1-abcdwxyz'
    let body: unknown
    server.use(
      http.patch('/api/v1/decider', async ({ request }) => {
        body = await request.json()
        return HttpResponse.json({ ...fx.decider, key: { source: 'file', name: '/x/decider.env', masked: '...wxyz' } })
      }),
    )
    const { queryClient } = await openDecider()
    await userEvent.click(screen.getByRole('radio', { name: 'Paste key' }))
    const key = screen.getByLabelText('Key') as HTMLInputElement
    expect(key).toHaveAttribute('type', 'password')
    expect(key).toHaveAttribute('autocomplete', 'new-password')
    expect(key).toHaveAttribute('data-1p-ignore')
    expect(key).toHaveAttribute('spellcheck', 'false')
    expect(key.value).toBe('')
    await userEvent.type(key, SECRET)
    expect(key.value).toBe(SECRET)
    await save()
    await waitFor(() => expect(body).toEqual({ key: { paste: SECRET } }))
    await waitFor(() => expect(screen.getByTestId('decider-key-current')).toHaveTextContent('...wxyz'))
    await userEvent.click(screen.getByRole('radio', { name: 'Paste key' }))
    expect((screen.getByLabelText('Key') as HTMLInputElement).value).toBe('')
    expect(JSON.stringify(queryClient.getMutationCache().getAll().map((m) => m.state.variables))).not.toContain(SECRET)
  })

  it('drops the pasted text when the key source leaves paste, and on Discard', async () => {
    await openDecider()
    await userEvent.click(screen.getByRole('radio', { name: 'Paste key' }))
    await userEvent.type(screen.getByLabelText('Key'), 'secret-1')
    await userEvent.click(screen.getByRole('radio', { name: 'Key file' }))
    await userEvent.click(screen.getByRole('radio', { name: 'Paste key' }))
    expect((screen.getByLabelText('Key') as HTMLInputElement).value).toBe('')
    await userEvent.type(screen.getByLabelText('Key'), 'secret-2')
    await userEvent.click(screen.getByRole('button', { name: 'Discard' }))
    await userEvent.click(screen.getByRole('radio', { name: 'Paste key' }))
    expect((screen.getByLabelText('Key') as HTMLInputElement).value).toBe('')
  })

  it('drops the pasted key after 4xx and 5xx failures, with no secret in the cache', async () => {
    let failure = { status: 422, message: 'key: invalid' }
    server.use(http.patch('/api/v1/decider', () => HttpResponse.json({ error: { code: -32602, message: failure.message } }, { status: failure.status })))
    const { queryClient } = await openDecider()
    const cached = () => JSON.stringify(queryClient.getMutationCache().getAll().map((m) => [m.state.variables, m.state.error, m.state.data]))
    await userEvent.click(screen.getByRole('radio', { name: 'Paste key' }))
    await userEvent.type(screen.getByLabelText('Key'), 'bad-key')
    await save()
    expect(await screen.findByText('key: invalid')).toBeInTheDocument()
    await waitFor(() => expect((screen.getByLabelText('Key') as HTMLInputElement).value).toBe(''))
    expect(cached()).not.toContain('bad-key')

    failure = { status: 500, message: 'supervisor unavailable, try again' }
    await userEvent.type(screen.getByLabelText('Key'), 'kept-key')
    await save()
    expect(await screen.findByText(/supervisor unavailable, try again/)).toBeInTheDocument()
    await waitFor(() => expect((screen.getByLabelText('Key') as HTMLInputElement).value).toBe(''))
    expect(cached()).not.toContain('kept-key')
  })

  it('clears the pasted key after a network failure', async () => {
    server.use(http.patch('/api/v1/decider', () => HttpResponse.error()))
    await openDecider()
    await userEvent.click(screen.getByRole('radio', { name: 'Paste key' }))
    await userEvent.type(screen.getByLabelText('Key'), 'network-secret')
    await save()
    await waitFor(() => expect(screen.getByLabelText('Key')).toHaveValue(''))
  })

  it('clears pasted keys when leaving the tab and on unmount', async () => {
    const { unmount } = await openDecider()
    await userEvent.click(screen.getByRole('radio', { name: 'Paste key' }))
    const key = screen.getByLabelText('Key') as HTMLInputElement
    await userEvent.type(key, 'tab-secret')
    await openTab('General')
    await openTab('Decider')
    expect(screen.getByLabelText('Key')).toHaveValue('')
    await userEvent.type(screen.getByLabelText('Key'), 'unmount-secret')
    const input = screen.getByLabelText('Key') as HTMLInputElement
    unmount()
    expect(input.value).toBe('')
  })

  it('never stores a pasted key in the mutation cache while pending', async () => {
    let release!: () => void
    const gate = new Promise<void>((resolve) => { release = resolve })
    let received = false
    server.use(http.patch('/api/v1/decider', async () => {
      received = true
      await gate
      return HttpResponse.json(fx.decider)
    }))
    const { queryClient } = await openDecider()
    const seen: string[] = []
    const unsubscribe = queryClient.getMutationCache().subscribe(() => {
      seen.push(JSON.stringify(queryClient.getMutationCache().getAll().map((m) => m.state)))
    })
    await userEvent.click(screen.getByRole('radio', { name: 'Paste key' }))
    await userEvent.type(screen.getByLabelText('Key'), 'pending-secret')
    await save()
    await waitFor(() => expect(received).toBe(true))
    release()
    await waitFor(() => expect(screen.getByTestId('decider-key-current')).toBeInTheDocument())
    unsubscribe()
    expect(seen.join()).not.toContain('pending-secret')
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
            { name: 'SUPERORCHESTRATOR_ROUTER_FALLBACK_MODEL', field: 'fallback_model' },
          ],
          effective: { ...fx.decider.effective, primary: 'x/forced', primary_source: 'env' },
        }),
      ),
    )
    await openDecider()
    expect(screen.getByTestId('env-override-model')).toHaveTextContent('SUPERORCHESTRATOR_ROUTER_MODEL is set (x/forced) and wins over this setting until it is unset.')
    expect(screen.getByTestId('env-override-fallback_model')).toHaveTextContent('SUPERORCHESTRATOR_ROUTER_FALLBACK_MODEL is set and wins')
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

  it('shows a server 422 on the named field', async () => {
    server.use(http.patch('/api/v1/decider', () => HttpResponse.json({ error: { code: -32602, message: 'model: unknown model id' } }, { status: 422 })))
    await openDecider()
    const model = screen.getByLabelText('Model')
    await userEvent.type(model, '-x')
    await save()
    expect(await screen.findByText('unknown model id')).toBeInTheDocument()
  })

  it('shows an unattributed server 422 as a form error', async () => {
    server.use(http.patch('/api/v1/decider', () => HttpResponse.json({ error: { code: -32602, message: 'config is locked' } }, { status: 422 })))
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
    const row = (name: string) => within(r).getByText(name).nextElementSibling
    expect(row('Complexity')).toHaveTextContent('62.5')
    expect(row('Level')).toHaveTextContent('high')
    expect(row('Confidence')).toHaveTextContent('0.81')
    expect(row('Model')).toHaveTextContent('openai/gpt-5-mini')
    expect(row('Latency')).toHaveTextContent('412 ms')
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

  it('shows a transport error without a latency and without a toast', async () => {
    server.use(http.post('/api/v1/decider/test', () => HttpResponse.error()))
    await openDecider()
    await userEvent.click(screen.getByRole('button', { name: 'Test decider' }))
    const alert = await screen.findByText(/Test failed:/)
    expect(alert).not.toHaveTextContent('ms')
    expect(toast.error).not.toHaveBeenCalled()
  })
})
