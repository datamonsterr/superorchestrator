import { renderHook, waitFor } from '@testing-library/react'
import { QueryClientProvider } from '@tanstack/react-query'
import { http, HttpResponse } from 'msw'
import type { ReactNode } from 'react'
import { describe, expect, it } from 'vitest'
import { server } from '@/test/server'
import { createTestQueryClient } from '@/test/render'
import { qk, usePollingInterval, useSessions, useSignInStatus, useStatus } from './queries'
import { StreamContext } from './stream'

const wrap = (polling = false) =>
  function Wrapper({ children }: { children: ReactNode }) {
    return (
      <QueryClientProvider client={createTestQueryClient()}>
        <StreamContext.Provider value={{ status: polling ? 'closed' : 'open', polling, bootId: null }}>{children}</StreamContext.Provider>
      </QueryClientProvider>
    )
  }

describe('query hooks', () => {
  it('useStatus returns the status with caps', async () => {
    const { result } = renderHook(() => useStatus(), { wrapper: wrap() })
    await waitFor(() => expect(result.current.data?.caps.global).toBe(8))
  })

  it('useSessions pages with the cursor', async () => {
    server.use(
      http.get('*/api/v1/sessions', ({ request }) => {
        const c = new URL(request.url).searchParams.get('cursor')
        return HttpResponse.json(c ? { items: [{ id: 's2' }], next_cursor: null } : { items: [{ id: 's1' }], next_cursor: 'c1' })
      }),
    )
    const { result } = renderHook(() => useSessions({}), { wrapper: wrap() })
    await waitFor(() => expect(result.current.hasNextPage).toBe(true))
    await result.current.fetchNextPage()
    await waitFor(() => expect(result.current.data?.pages).toHaveLength(2))
    expect(result.current.hasNextPage).toBe(false)
  })

  it('polls only when the stream is down', () => {
    expect(renderHook(() => usePollingInterval(), { wrapper: wrap(false) }).result.current).toBe(false)
    expect(renderHook(() => usePollingInterval(), { wrapper: wrap(true) }).result.current).toBe(5000)
  })

  it('sign-in status stops polling once done', async () => {
    let calls = 0
    server.use(
      http.get('*/api/v1/accounts/:name/login', () => {
        calls++
        return HttpResponse.json({ signed_in: 'signed_in', mode: 'manual', state: 'done' })
      }),
    )
    const { result } = renderHook(() => useSignInStatus('agy-acc1'), { wrapper: wrap() })
    await waitFor(() => expect(result.current.data?.state).toBe('done'))
    const before = calls
    await new Promise((r) => setTimeout(r, 2300))
    expect(calls).toBe(before)
  })

  it('exposes the design 2.2 key shapes', () => {
    expect(qk.session('s')).toEqual(['session', 's'])
    expect(qk.sessionEvents('s')).toEqual(['session', 's', 'events'])
    expect(qk.signin('a')).toEqual(['accounts', 'a', 'signin'])
    expect(qk.usageDaily(30)).toEqual(['usage', 'daily', 30])
    expect(qk.runRoute('r')).toEqual(['run', 'r', 'route'])
  })
})
