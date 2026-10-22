import { act, screen } from '@testing-library/react'
import { http, HttpResponse } from 'msw'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { server } from '@/test/server'
import { createTestQueryClient, renderWithProviders } from '@/test/render'
import { qk } from '@/api/queries'
import * as fx from '@/api/msw/fixtures'
import type { SignInStatus } from '@/api/types'
import { SignInDialog } from './SignInDialog'

// Not faked below, so MSW's real I/O can complete while the clock stands still.
const realImmediate = (globalThis as unknown as { setImmediate: (f: () => void) => void }).setImmediate
const NOW = Date.UTC(2026, 0, 1, 12, 0, 0)

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'setInterval', 'clearTimeout', 'clearInterval', 'Date'], now: NOW, shouldAdvanceTime: false })
})
afterEach(() => vi.useRealTimers())

/** Lets MSW and React Query settle without moving the clock far. */
const settle = async () => {
  // MSW answers on real I/O, so give the event loop a few turns while the fake clock barely moves.
  for (let i = 0; i < 5; i++) {
    await act(async () => {
      await new Promise<void>((r) => realImmediate(r))
      await vi.advanceTimersByTimeAsync(10)
    })
  }
}
const tick = (ms: number) => act(() => vi.advanceTimersByTimeAsync(ms))

function pollServer(sequence: SignInStatus[]) {
  let polls = 0
  server.use(
    http.get('/api/v1/accounts/:name/login', () => HttpResponse.json(sequence[Math.min(polls++, sequence.length - 1)])),
  )
  return () => polls
}
const pending: SignInStatus = { signed_in: 'signed_out', mode: 'device_auth', state: 'pending' }

describe('SignInDialog', () => {
  it('shows the device-auth url, code and countdown, polls every 2 s and closes on done', async () => {
    const polls = pollServer([pending, pending, { ...pending, signed_in: 'signed_in', state: 'done' }])
    const onClose = vi.fn()
    renderWithProviders(<SignInDialog name="agy-acc1" onClose={onClose} />)
    await settle()
    expect(screen.getByLabelText('One-time code')).toHaveTextContent('ABCD-1234')
    expect(screen.getByRole('link', { name: /Open sign-in page/ })).toHaveAttribute('href', 'https://example.test/device')
    expect(screen.getByRole('link', { name: /Open sign-in page/ })).toHaveAttribute('rel', 'noreferrer')
    expect(screen.getByText('Expires in 15:00')).toBeInTheDocument()
    expect(screen.getByText('Waiting for sign-in')).toBeInTheDocument()
    expect(polls()).toBe(1)

    await tick(2000)
    expect(polls()).toBe(2)
    expect(onClose).not.toHaveBeenCalled()
    await tick(2000)
    expect(polls()).toBe(3)
    expect(onClose).toHaveBeenCalledOnce()

    // Polling stops once done.
    await tick(10_000)
    expect(polls()).toBe(3)
  })

  it('stops polling when the dialog closes', async () => {
    const polls = pollServer([pending])
    const { rerender, queryClient } = renderWithProviders(<SignInDialog name="agy-acc1" onClose={() => {}} />)
    void queryClient
    await settle()
    await tick(2000)
    const before = polls()
    expect(before).toBeGreaterThan(1)
    rerender(<SignInDialog name={null} onClose={() => {}} />)
    await tick(10_000)
    expect(polls()).toBe(before)
  })

  it('shows the command for manual sign-in', async () => {
    server.use(
      http.post('/api/v1/accounts/:name/login', () => HttpResponse.json({ mode: 'manual', command: 'agy auth login', cwd: '/home/me/.agy-acc1' })),
    )
    pollServer([{ signed_in: 'signed_out', mode: 'manual', state: 'pending' }])
    renderWithProviders(<SignInDialog name="agy-acc1" onClose={() => {}} />)
    await settle()
    expect(screen.getByText('cd /home/me/.agy-acc1 && agy auth login')).toBeInTheDocument()
    expect(screen.getByText('Run this in a terminal, then come back.')).toBeInTheDocument()
    expect(screen.getByText('Waiting for sign-in')).toBeInTheDocument()
  })

  it('offers Try again after a failure and re-posts the login', async () => {
    let logins = 0
    server.use(
      http.post('/api/v1/accounts/:name/login', () => {
        logins++
        return HttpResponse.json({ mode: 'manual', command: 'agy auth login' })
      }),
    )
    pollServer([{ signed_in: 'signed_out', mode: 'manual', state: 'failed', message: 'denied by user' }, pending])
    renderWithProviders(<SignInDialog name="agy-acc1" onClose={() => {}} />)
    await settle()
    expect(screen.getByRole('alert')).toHaveTextContent('denied by user')
    expect(logins).toBe(1)
    const retry = screen.getByRole('button', { name: 'Try again' })
    await act(async () => retry.click())
    await settle()
    expect(logins).toBe(2)
    expect(screen.queryByRole('alert')).not.toBeInTheDocument()
  })

  it('reports an expired device code', async () => {
    pollServer([pending])
    renderWithProviders(<SignInDialog name="agy-acc1" onClose={() => {}} />)
    await settle()
    await tick(16 * 60 * 1000)
    expect(screen.getByText('The code has expired.')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Try again' })).toBeInTheDocument()
  })

  it('finishes early when the account stream event marks it signed in', async () => {
    pollServer([pending])
    const onClose = vi.fn()
    const client = createTestQueryClient()
    client.setQueryData(qk.accounts(), fx.accounts) // agy-acc1 starts signed out
    renderWithProviders(<SignInDialog name="agy-acc1" onClose={onClose} />, { queryClient: client })
    await settle()
    expect(onClose).not.toHaveBeenCalled()
    act(() => client.setQueryData(qk.accounts(), fx.accounts.map((a) => (a.name === 'agy-acc1' ? { ...a, signed_in: 'signed_in' as const } : a))))
    await settle()
    expect(onClose).toHaveBeenCalledOnce()
  })
})
