import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import type { QueryClient } from '@tanstack/react-query'
import { http, HttpResponse } from 'msw'
import { describe, expect, it } from 'vitest'
import { server } from '@/test/server'
import { renderWithProviders } from '@/test/render'
import * as fx from '@/api/msw/fixtures'
import { AccountsPage } from './AccountsPage'

const row = (name: string) => screen.findByRole('row', { name: new RegExp(name) })
async function openMenu(name: string, item: string) {
  await userEvent.click(await screen.findByRole('button', { name: `Actions for ${name}` }))
  await userEvent.click(await screen.findByRole('menuitem', { name: item }))
}

describe('AccountsPage', () => {
  it('marks reserved accounts', async () => {
    server.use(http.get('/api/v1/accounts', () => HttpResponse.json(fx.accounts.map((a, i) => ({ ...a, reserved: i === 0 })))))
    renderWithProviders(<AccountsPage />)
    expect(within(await row('codex-acc1')).getByTitle('Kept for the orchestrator; auto routing skips it')).toHaveTextContent('reserved')
    expect(within(await row('agy-acc1')).queryByText('reserved')).not.toBeInTheDocument()
  })
  it('lists accounts with sign-in, pools, cooldown and headroom', async () => {
    renderWithProviders(<AccountsPage />)
    const agy = await row('agy-acc1')
    expect(within(agy).getByText('Signed out')).toBeInTheDocument()
    expect(within(agy).getAllByText('gemini').length).toBeGreaterThan(0)
    expect(within(agy).getByText(/cooldown claude/)).toBeInTheDocument()
    expect(within(agy).getAllByRole('progressbar').length).toBeGreaterThan(0)
    expect(within(agy).getByRole('button', { name: /Sign in/ })).toBeInTheDocument()
    expect(within(await row('codex-acc1')).queryByRole('button', { name: /^Sign in/ })).not.toBeInTheDocument()
  })

  it('shows the empty state and error state', async () => {
    server.use(http.get('/api/v1/accounts', () => HttpResponse.json([])))
    const { unmount } = renderWithProviders(<AccountsPage />)
    expect(await screen.findByText('Add your first account')).toBeInTheDocument()
    unmount()
    server.use(http.get('/api/v1/accounts', () => HttpResponse.json({ error: { code: -32004, message: 'down' } }, { status: 503 })))
    renderWithProviders(<AccountsPage />)
    expect(await screen.findByRole('alert')).toHaveTextContent('down')
  })

  it('patches enabled from the switch', async () => {
    let body: unknown
    server.use(
      http.patch('/api/v1/accounts/:name', async ({ request, params }) => {
        body = { name: params.name, ...((await request.json()) as object) }
        return HttpResponse.json(fx.accounts[0])
      }),
    )
    renderWithProviders(<AccountsPage />)
    await userEvent.click(await screen.findByRole('switch', { name: 'Enable codex-acc1' }))
    await waitFor(() => expect(body).toEqual({ name: 'codex-acc1', enabled: false }))
  })

  describe('remove', () => {
    it('requires typing the account name and sends purge', async () => {
      let url = ''
      server.use(
        http.delete('/api/v1/accounts/:name', ({ request }) => {
          url = request.url
          return HttpResponse.json({ ok: true })
        }),
      )
      renderWithProviders(<AccountsPage />)
      await openMenu('codex-acc1', 'Remove')
      const dialog = await screen.findByRole('alertdialog')
      const confirm = within(dialog).getByRole('button', { name: 'Remove account' })
      expect(confirm).toBeDisabled()
      expect(within(dialog).getByRole('checkbox', { name: /delete the credential home/ })).not.toBeChecked()
      await userEvent.type(within(dialog).getByRole('textbox'), 'codex-acc')
      expect(confirm).toBeDisabled()
      await userEvent.type(within(dialog).getByRole('textbox'), '1')
      expect(confirm).toBeEnabled()
      await userEvent.click(within(dialog).getByRole('checkbox', { name: /delete the credential home/ }))
      expect(within(dialog).getByText(/permanently deletes/)).toBeInTheDocument()
      await userEvent.click(confirm)
      await waitFor(() => expect(url).toMatch(/\/accounts\/codex-acc1\?purge=true$/))
      await waitFor(() => expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument())
    })

    it('does not delete when cancelled', async () => {
      let called = false
      server.use(http.delete('/api/v1/accounts/:name', () => ((called = true), HttpResponse.json({ ok: true }))))
      renderWithProviders(<AccountsPage />)
      await openMenu('codex-acc1', 'Remove')
      await userEvent.click(await screen.findByRole('button', { name: 'Cancel' }))
      expect(called).toBe(false)
    })
  })

  describe('add', () => {
    it('validates the name, creates the account and opens sign-in', async () => {
      let created: unknown
      server.use(
        http.post('/api/v1/accounts', async ({ request }) => {
          created = await request.json()
          return HttpResponse.json({ ...fx.accounts[0], name: 'codex-acc2', signed_in: 'signed_out' }, { status: 201 })
        }),
      )
      renderWithProviders(<AccountsPage />)
      await userEvent.click(await screen.findByRole('button', { name: /Add account/ }))
      const dialog = await screen.findByRole('dialog', { name: 'Add account' })
      await userEvent.type(within(dialog).getByLabelText('Name'), 'Bad Name')
      await userEvent.click(within(dialog).getByRole('button', { name: 'Add and sign in' }))
      expect(await within(dialog).findByRole('alert')).toHaveTextContent('Use lowercase letters')
      expect(created).toBeUndefined()

      await userEvent.clear(within(dialog).getByLabelText('Name'))
      await userEvent.type(within(dialog).getByLabelText('Name'), 'codex-acc1')
      await userEvent.click(within(dialog).getByRole('button', { name: 'Add and sign in' }))
      await waitFor(() => expect(within(dialog).getByRole('alert')).toHaveTextContent(/already exists/))

      await userEvent.clear(within(dialog).getByLabelText('Name'))
      await userEvent.type(within(dialog).getByLabelText('Name'), 'codex-acc2')
      await userEvent.click(within(dialog).getByRole('button', { name: 'Add and sign in' }))
      expect(await screen.findByRole('dialog', { name: 'Sign in codex-acc2' })).toBeInTheDocument()
      expect(created).toEqual({ harness: 'codex', name: 'codex-acc2' })
      expect(await screen.findByLabelText('One-time code')).toHaveTextContent('ABCD-1234')
    })

    it('clears the form when the dialog is closed and reopened', async () => {
      renderWithProviders(<AccountsPage />)
      await userEvent.click(await screen.findByRole('button', { name: /Add account/ }))
      await userEvent.type(await screen.findByLabelText('Name'), 'half-typed')
      await userEvent.click(screen.getByRole('button', { name: 'Cancel' }))
      await waitFor(() => expect(screen.queryByRole('dialog', { name: 'Add account' })).not.toBeInTheDocument())
      await userEvent.click(screen.getByRole('button', { name: /Add account/ }))
      expect(await screen.findByLabelText('Name')).toHaveValue('')
    })

    it('blocks a second agy account on macOS', async () => {
      server.use(http.get('/api/v1/config', () => HttpResponse.json({ ...fx.config, platform: 'macos' })))
      renderWithProviders(<AccountsPage />)
      await row('agy-acc1')
      await userEvent.click(screen.getByRole('button', { name: /Add account/ }))
      await userEvent.click(await screen.findByRole('combobox', { name: 'Harness' }))
      expect(await screen.findByRole('option', { name: /Antigravity.*one per macOS/ })).toHaveAttribute('aria-disabled', 'true')
    })
  })

  describe('endpoint accounts', () => {
    let queryClient: QueryClient
    async function openAdd() {
      queryClient = renderWithProviders(<AccountsPage />).queryClient
      await userEvent.click(await screen.findByRole('button', { name: /Add account/ }))
      const dialog = await screen.findByRole('dialog', { name: 'Add account' })
      await userEvent.click(within(dialog).getByRole('combobox', { name: 'Harness' }))
      await userEvent.click(await screen.findByRole('option', { name: 'Claude' }))
      await userEvent.type(within(dialog).getByLabelText('Name'), 'zai-1')
      return dialog
    }
    async function pickPreset(dialog: HTMLElement, label: string) {
      await userEvent.click(within(dialog).getByRole('combobox', { name: 'Endpoint' }))
      await userEvent.click(await screen.findByRole('option', { name: label }))
    }
    const capture = (sink: { body?: unknown }) =>
      server.use(
        http.post('/api/v1/accounts', async ({ request }) => {
          sink.body = await request.json()
          return HttpResponse.json({ ...fx.accounts[0], name: 'zai-1', harness: 'claude', signed_in: 'signed_out' }, { status: 201 })
        }),
      )

    it('hides endpoint fields for other harnesses and for None', async () => {
      renderWithProviders(<AccountsPage />)
      await userEvent.click(await screen.findByRole('button', { name: /Add account/ }))
      const dialog = await screen.findByRole('dialog', { name: 'Add account' })
      expect(within(dialog).queryByRole('combobox', { name: 'Endpoint' })).not.toBeInTheDocument()
      await userEvent.click(within(dialog).getByRole('combobox', { name: 'Harness' }))
      await userEvent.click(await screen.findByRole('option', { name: 'Claude' }))
      expect(within(dialog).getByRole('combobox', { name: 'Endpoint' })).toHaveTextContent('None')
      expect(within(dialog).queryByLabelText('API key')).not.toBeInTheDocument()
    })

    it('sends a preset with a pasted key and shows the read-only base URL', async () => {
      const sink: { body?: unknown } = {}
      capture(sink)
      const dialog = await openAdd()
      await pickPreset(dialog, 'Z.ai GLM')
      const base = within(dialog).getByLabelText('Base URL')
      expect(base).toHaveValue('https://api.z.ai/api/anthropic')
      expect(base).toHaveAttribute('readonly')
      const key = within(dialog).getByLabelText('API key', { selector: 'input' })
      expect(key).toHaveAttribute('type', 'password')
      expect(key).toHaveAttribute('autocomplete', 'off')
      await userEvent.type(key, 'sk-secret')
      await userEvent.click(within(dialog).getByRole('button', { name: 'Add and sign in' }))
      await waitFor(() => expect(sink.body).toEqual({ harness: 'claude', name: 'zai-1', endpoint: { preset: 'zai' }, token: 'sk-secret' }))
    })

    it('requires a key and sends token_env without a token for the env variant', async () => {
      const sink: { body?: unknown } = {}
      capture(sink)
      const dialog = await openAdd()
      await pickPreset(dialog, 'Kimi Code')
      await userEvent.click(within(dialog).getByRole('button', { name: 'Add and sign in' }))
      expect(await within(dialog).findByText('Paste the API key')).toBeInTheDocument()
      expect(sink.body).toBeUndefined()
      await userEvent.click(within(dialog).getByRole('radio', { name: 'Environment variable' }))
      await userEvent.type(within(dialog).getByLabelText('Variable name'), 'KIMI_API_KEY')
      await userEvent.click(within(dialog).getByRole('button', { name: 'Add and sign in' }))
      await waitFor(() => expect(sink.body).toEqual({ harness: 'claude', name: 'zai-1', endpoint: { preset: 'kimi', token_env: 'KIMI_API_KEY' } }))
    })

    it('custom shows and requires its fields', async () => {
      const sink: { body?: unknown } = {}
      capture(sink)
      const dialog = await openAdd()
      await pickPreset(dialog, 'Custom')
      await userEvent.type(within(dialog).getByLabelText('API key', { selector: 'input' }), 'k')
      await userEvent.click(within(dialog).getByRole('button', { name: 'Add and sign in' }))
      expect(await within(dialog).findByText('Enter the base URL')).toBeInTheDocument()
      expect(within(dialog).getByText('Enter at least one model')).toBeInTheDocument()
      expect(within(dialog).getByText('Enter the haiku model')).toBeInTheDocument()
      expect(sink.body).toBeUndefined()
      await userEvent.type(within(dialog).getByLabelText('Base URL'), 'https://llm.example.com/v1')
      await userEvent.type(within(dialog).getByLabelText('Models'), 'm1, m2')
      await userEvent.type(within(dialog).getByLabelText('Opus model'), 'm1')
      await userEvent.type(within(dialog).getByLabelText('Sonnet model'), 'm1')
      await userEvent.type(within(dialog).getByLabelText('Haiku model'), 'm2')
      await userEvent.click(within(dialog).getByRole('button', { name: 'Add and sign in' }))
      await waitFor(() =>
        expect(sink.body).toEqual({
          harness: 'claude',
          name: 'zai-1',
          endpoint: { preset: 'custom', base_url: 'https://llm.example.com/v1', models: ['m1', 'm2'], slots: { opus: 'm1', sonnet: 'm1', haiku: 'm2' } },
          token: 'k',
        }),
      )
    })

    it('does not keep the key after close and reopen', async () => {
      const dialog = await openAdd()
      await pickPreset(dialog, 'MiniMax')
      await userEvent.type(within(dialog).getByLabelText('API key', { selector: 'input' }), 'sk-secret')
      await userEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }))
      await waitFor(() => expect(screen.queryByRole('dialog', { name: 'Add account' })).not.toBeInTheDocument())
      await userEvent.click(screen.getByRole('button', { name: /Add account/ }))
      const again = await screen.findByRole('dialog', { name: 'Add account' })
      await userEvent.click(within(again).getByRole('combobox', { name: 'Harness' }))
      await userEvent.click(await screen.findByRole('option', { name: 'Claude' }))
      await pickPreset(again, 'MiniMax')
      expect(within(again).getByRole('radio', { name: 'Paste key' })).toBeChecked()
      expect(within(again).getByLabelText('API key', { selector: 'input' })).toHaveValue('')
      expect(screen.queryByDisplayValue('sk-secret')).not.toBeInTheDocument()
    })

    it.each([
      ['switching the key source', async (d: HTMLElement) => {
        await userEvent.click(within(d).getByRole('radio', { name: 'Environment variable' }))
        await userEvent.click(within(d).getByRole('radio', { name: 'Paste key' }))
      }],
      ['changing the preset', async (d: HTMLElement) => {
        await pickPreset(d, 'Kimi Code')
      }],
      ['changing the harness', async (d: HTMLElement) => {
        await userEvent.click(within(d).getByRole('combobox', { name: 'Harness' }))
        await userEvent.click(await screen.findByRole('option', { name: 'Codex' }))
        await userEvent.click(within(d).getByRole('combobox', { name: 'Harness' }))
        await userEvent.click(await screen.findByRole('option', { name: 'Claude' }))
      }],
    ])('clears the pasted key when %s', async (_n, change) => {
      const dialog = await openAdd()
      await pickPreset(dialog, 'Z.ai GLM')
      await userEvent.type(within(dialog).getByLabelText('API key', { selector: 'input' }), 'sk-secret')
      await change(dialog)
      if (within(dialog).queryByLabelText('API key', { selector: 'input' }) === null) await pickPreset(dialog, 'Z.ai GLM')
      expect(within(dialog).getByLabelText('API key', { selector: 'input' })).toHaveValue('')
    })

    it('sends the key trimmed', async () => {
      const sink: { body?: unknown } = {}
      capture(sink)
      const dialog = await openAdd()
      await pickPreset(dialog, 'Z.ai GLM')
      await userEvent.type(within(dialog).getByLabelText('API key', { selector: 'input' }), '  sk-1  ')
      await userEvent.click(within(dialog).getByRole('button', { name: 'Add and sign in' }))
      await waitFor(() => expect(sink.body).toMatchObject({ token: 'sk-1' }))
    })

    it('leaves no API key in the mutation cache after submit', async () => {
      const sink: { body?: unknown } = {}
      capture(sink)
      const dialog = await openAdd()
      await pickPreset(dialog, 'Z.ai GLM')
      await userEvent.type(within(dialog).getByLabelText('API key', { selector: 'input' }), 'sk-secret')
      await userEvent.click(within(dialog).getByRole('button', { name: 'Add and sign in' }))
      await waitFor(() => expect(sink.body).toBeDefined())
      await screen.findByRole('dialog', { name: 'Sign in zai-1' })
      await waitFor(() => expect(JSON.stringify(queryClient.getMutationCache().getAll().map((m) => m.state.variables))).not.toContain('sk-secret'))
    })

    it('renders API field errors', async () => {
      server.use(
        http.post('/api/v1/accounts', () =>
          HttpResponse.json(
            { error: { code: -32602, message: 'bad', data: { 'endpoint.token_env': 'not set in the daemon environment', 'endpoint.base_url': 'unreachable', token: 'rejected' } } },
            { status: 422 },
          ),
        ),
      )
      const dialog = await openAdd()
      await pickPreset(dialog, 'Z.ai GLM')
      await userEvent.type(within(dialog).getByLabelText('API key', { selector: 'input' }), 'k')
      await userEvent.click(within(dialog).getByRole('button', { name: 'Add and sign in' }))
      expect(await within(dialog).findByText('rejected')).toBeInTheDocument()
      expect(within(dialog).getByText('unreachable')).toBeInTheDocument()
      expect(within(dialog).getByLabelText('API key', { selector: 'input' })).toHaveValue('')
    })

    it('shows slot and hidden-field server errors in a form-level message', async () => {
      server.use(
        http.post('/api/v1/accounts', () =>
          HttpResponse.json({ error: { code: -32602, message: 'bad', data: { 'endpoint.slots': 'opus is not in models', 'endpoint.token_env': 'unset' } } }, { status: 422 }),
        ),
      )
      const dialog = await openAdd()
      await pickPreset(dialog, 'Z.ai GLM')
      await userEvent.type(within(dialog).getByLabelText('API key', { selector: 'input' }), 'k')
      await userEvent.click(within(dialog).getByRole('button', { name: 'Add and sign in' }))
      expect(await within(dialog).findByRole('alert')).toHaveTextContent(/opus is not in models|unset/)
      expect(within(dialog).queryByLabelText('Opus model')).not.toBeInTheDocument()
    })

    it('shows the preset badge and token source for endpoint accounts', async () => {
      const ep = { preset: 'zai', base_url: 'https://api.z.ai/api/anthropic', models: ['glm-4.6'], token_source: 'env:ZAI_API_KEY' }
      server.use(http.get('/api/v1/accounts', () => HttpResponse.json([{ ...fx.accounts[0], name: 'zai-1', harness: 'claude', endpoint: ep }, fx.accounts[0]])))
      renderWithProviders(<AccountsPage />)
      expect(within(await row('zai-1')).getByText('Z.ai')).toBeInTheDocument()
      await openMenu('zai-1', 'Edit')
      expect(await screen.findByText(/Endpoint: Z\.ai GLM/)).toBeInTheDocument()
      expect(await screen.findByText(/API key source: env:ZAI_API_KEY/)).toBeInTheDocument()
    })
  })

  it('imports scanned accounts with per-row status', async () => {
    let imported: unknown
    server.use(
      http.post('/api/v1/accounts/import', async ({ request }) => {
        imported = await request.json()
        return HttpResponse.json(fx.accounts[0])
      }),
    )
    renderWithProviders(<AccountsPage />)
    await userEvent.click(await screen.findByRole('button', { name: /Scan and import/ }))
    const dialog = await screen.findByRole('dialog', { name: 'Scan and import' })
    const importBtn = within(dialog).getByRole('button', { name: /Import selected/ })
    expect(importBtn).toBeDisabled()
    await userEvent.click(await within(dialog).findByRole('checkbox', { name: 'Import codex-acc9' }))
    await userEvent.click(importBtn)
    expect(await within(dialog).findByText('Imported')).toBeInTheDocument()
    expect(imported).toEqual({ name: 'codex-acc9', harness: 'codex', home: '/home/me/.codex-acc9' })
  })

  describe('edit', () => {
    it('validates the cap and saves', async () => {
      let body: unknown
      server.use(
        http.patch('/api/v1/accounts/:name', async ({ request }) => {
          body = await request.json()
          return HttpResponse.json(fx.accounts[0])
        }),
      )
      renderWithProviders(<AccountsPage />)
      await openMenu('codex-acc1', 'Edit')
      const sheet = await screen.findByRole('dialog', { name: 'Edit codex-acc1' })
      const cap = within(sheet).getByLabelText('Concurrency cap')
      expect(cap).toHaveValue(2)
      await userEvent.clear(cap)
      await userEvent.type(cap, '20')
      await userEvent.click(within(sheet).getByRole('button', { name: 'Save' }))
      expect(await within(sheet).findByText('At most 16')).toBeInTheDocument()
      expect(body).toBeUndefined()
      await userEvent.clear(cap)
      await userEvent.type(cap, '4')
      await userEvent.click(within(sheet).getByRole('button', { name: 'Save' }))
      await waitFor(() => expect(body).toEqual({ cap: 4, enabled: true }))
    })

    it('offers pool checkboxes for agy', async () => {
      let body: { pools?: string[] } = {}
      server.use(
        http.patch('/api/v1/accounts/:name', async ({ request }) => {
          body = (await request.json()) as typeof body
          return HttpResponse.json(fx.accounts[1])
        }),
      )
      renderWithProviders(<AccountsPage />)
      await openMenu('agy-acc1', 'Edit')
      const sheet = await screen.findByRole('dialog', { name: 'Edit agy-acc1' })
      await userEvent.click(within(sheet).getByRole('checkbox', { name: 'claude' }))
      await userEvent.click(within(sheet).getByRole('button', { name: 'Save' }))
      await waitFor(() => expect(body.pools).toEqual(['gemini']))
    })
  })

  it('flags a cooldown and can clear an existing flag', async () => {
    const bodies: unknown[] = []
    server.use(
      http.post('/api/v1/accounts/:name/flag', async ({ request }) => {
        bodies.push(await request.json())
        return HttpResponse.json(fx.accounts[1])
      }),
    )
    renderWithProviders(<AccountsPage />)
    await openMenu('agy-acc1', 'Flag cooldown')
    const dialog = await screen.findByRole('dialog', { name: /Flag cooldown for agy-acc1/ })
    await userEvent.type(within(dialog).getByLabelText('Reason (optional)'), 'quota')
    await userEvent.click(within(dialog).getByRole('button', { name: 'Flag account' }))
    await waitFor(() => expect(bodies[0]).toEqual({ hours: 5, reason: 'quota' }))

    await openMenu('agy-acc1', 'Flag cooldown')
    const again = await screen.findByRole('dialog', { name: /Flag cooldown for agy-acc1/ })
    await userEvent.click(within(within(again).getByRole('list', { name: 'Current flags' })).getByRole('button', { name: 'Clear' }))
    await waitFor(() => expect(bodies[1]).toEqual({ clear: true, pool: 'claude' }))
  })

  it('keeps the flag dialog open and shows the error when flagging fails', async () => {
    server.use(
      http.post('/api/v1/accounts/:name/flag', () => HttpResponse.json({ error: { code: -32602, message: 'bad until', data: { until: 'must be in the future' } } }, { status: 422 })),
    )
    renderWithProviders(<AccountsPage />)
    await openMenu('agy-acc1', 'Flag cooldown')
    const dialog = await screen.findByRole('dialog', { name: /Flag cooldown for agy-acc1/ })
    await userEvent.click(within(dialog).getByRole('button', { name: 'Flag account' }))
    expect(await within(dialog).findByRole('alert')).toHaveTextContent(/future|bad until/)
    expect(screen.getByRole('dialog', { name: /Flag cooldown for agy-acc1/ })).toBeInTheDocument()
  })

  it('keeps the flag dialog open when clearing fails', async () => {
    server.use(http.post('/api/v1/accounts/:name/flag', () => HttpResponse.json({ error: { code: -32004, message: 'down' } }, { status: 503 })))
    renderWithProviders(<AccountsPage />)
    await openMenu('agy-acc1', 'Flag cooldown')
    const dialog = await screen.findByRole('dialog', { name: /Flag cooldown for agy-acc1/ })
    await userEvent.click(within(within(dialog).getByRole('list', { name: 'Current flags' })).getByRole('button', { name: 'Clear' }))
    expect(await within(dialog).findByRole('alert')).toHaveTextContent('down')
    expect(screen.getByRole('dialog', { name: /Flag cooldown for agy-acc1/ })).toBeInTheDocument()
  })
})
