import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
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
