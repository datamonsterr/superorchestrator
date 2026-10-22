import { renderHook, waitFor } from '@testing-library/react'
import { http, HttpResponse } from 'msw'
import { describe, expect, it, vi } from 'vitest'
import { server } from '@/test/server'
import { useTranscriptLines } from './hooks'

const B = '/api/v1/runs/r1/transcript'
const line = (offset: number, text: string) => ({ offset, role: 'assistant' as const, text })

describe('useTranscriptLines', () => {
  it('does not re-render on tail polls that bring nothing new', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    let polls = 0
    server.use(
      http.get(B, ({ request }) => {
        const offset = Number(new URL(request.url).searchParams.get('offset'))
        if (offset > 0) polls++
        return HttpResponse.json(offset === 0 ? { lines: [line(0, 'a')], next_offset: null, total_bytes: 2 } : { lines: [], next_offset: null, total_bytes: 2 })
      }),
    )
    let renders = 0
    const { result, unmount } = renderHook(() => {
      renders++
      return useTranscriptLines('r1', true)
    })
    await waitFor(() => expect(result.current.lines).toHaveLength(1))
    const before = renders
    await vi.advanceTimersByTimeAsync(7000) // three polls
    expect(polls).toBeGreaterThanOrEqual(2)
    expect(renders).toBeLessThanOrEqual(before + 1) // React may render once more before bailing out; not once per poll
    unmount()
    vi.useRealTimers()
  })

  it('aborts the request in flight when unmounted', async () => {
    let aborted = false
    server.use(
      http.get(B, async ({ request }) => {
        request.signal.addEventListener('abort', () => (aborted = true))
        await new Promise((r) => setTimeout(r, 300))
        return HttpResponse.json({ lines: [], next_offset: null, total_bytes: 0 })
      }),
    )
    const { unmount } = renderHook(() => useTranscriptLines('r1', false))
    await new Promise((r) => setTimeout(r, 50))
    unmount()
    await waitFor(() => expect(aborted).toBe(true))
  })

  it('appends tail lines without duplicating earlier ones', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    let n = 0
    server.use(
      http.get(B, ({ request }) => {
        const offset = Number(new URL(request.url).searchParams.get('offset'))
        if (offset === 0) return HttpResponse.json({ lines: [line(0, 'a')], next_offset: null, total_bytes: 2 })
        n++
        return HttpResponse.json({ lines: [line(2, 'b')], next_offset: null, total_bytes: 4 })
      }),
    )
    const { result, unmount } = renderHook(() => useTranscriptLines('r1', true))
    await waitFor(() => expect(result.current.lines).toHaveLength(1))
    await vi.advanceTimersByTimeAsync(5000)
    expect(n).toBeGreaterThan(0)
    expect(result.current.lines.map((l) => l.text)).toEqual(['a', 'b'])
    unmount()
    vi.useRealTimers()
  })
})
