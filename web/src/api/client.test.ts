import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { server } from '@/test/server'
import { ApiError, buildUrl, request, setUnauthorizedHandler } from './client'
import { endpoints } from './endpoints'

describe('buildUrl', () => {
  it('drops empty values and encodes the rest', () => {
    expect(buildUrl('/sessions', { status: 'active', q: 'a b', cursor: null, limit: undefined, n: 0 })).toBe(
      '/api/v1/sessions?status=active&q=a+b&n=0',
    )
    expect(buildUrl('/status')).toBe('/api/v1/status')
  })
})

describe('request', () => {
  let restore = () => {}
  afterEach(() => restore())

  it('maps the error body to ApiError', async () => {
    server.use(
      http.get('*/api/v1/runs/x', () =>
        HttpResponse.json({ error: { code: -32003, message: 'run not found', data: { id: 'x' } } }, { status: 404 }),
      ),
    )
    const err = await endpoints.getRun('x').catch((e: unknown) => e)
    expect(err).toBeInstanceOf(ApiError)
    expect(err).toMatchObject({ status: 404, code: -32003, message: 'run not found', data: { id: 'x' } })
    expect((err as ApiError).isNotFound).toBe(true)
  })

  it('falls back to the status text for non-JSON errors', async () => {
    server.use(http.get('*/api/v1/status', () => new HttpResponse('boom', { status: 502, statusText: 'Bad Gateway' })))
    await expect(endpoints.getStatus()).rejects.toMatchObject({ status: 502, code: 0, message: 'Bad Gateway' })
  })

  it('flags 503 and 501 helpers', async () => {
    server.use(http.get('*/api/v1/doctor', () => HttpResponse.json({ error: { code: -32004, message: 'down' } }, { status: 503 })))
    const e = (await endpoints.getDoctor().catch((x: unknown) => x)) as ApiError
    expect(e.isUnavailable).toBe(true)
    expect(new ApiError(501, 0, 'x').isNotAvailable).toBe(true)
  })

  it('calls the unauthorized handler on 401 and still throws', async () => {
    const handler = vi.fn()
    restore = setUnauthorizedHandler(handler)
    server.use(http.get('*/api/v1/status', () => HttpResponse.json({ error: { code: -32000, message: 'no' } }, { status: 401 })))
    await expect(endpoints.getStatus()).rejects.toMatchObject({ status: 401, isUnauthorized: true })
    expect(handler).toHaveBeenCalledTimes(1)
  })

  it('wraps network failures in a status 0 ApiError', async () => {
    server.use(http.get('*/api/v1/status', () => HttpResponse.error()))
    await expect(endpoints.getStatus()).rejects.toMatchObject({ status: 0, name: 'ApiError' })
  })

  it('returns undefined for 204 and sends JSON content type on writes', async () => {
    let ct: string | null = null
    let body: unknown
    server.use(
      http.post('*/api/v1/auth', async ({ request: r }) => {
        ct = r.headers.get('content-type')
        body = await r.json()
        return new HttpResponse(null, { status: 204 })
      }),
      http.post('*/api/v1/runs/r1/cancel', ({ request: r }) => {
        ct = r.headers.get('content-type')
        return HttpResponse.json({ ok: true })
      }),
    )
    await expect(endpoints.postAuth('tok')).resolves.toBeUndefined()
    expect(ct).toContain('application/json')
    expect(body).toEqual({ token: 'tok' })
    await request('/runs/r1/cancel', { method: 'POST' })
    expect(ct).toContain('application/json')
  })

  it('serialises query params for list endpoints', async () => {
    let url = ''
    server.use(
      http.get('*/api/v1/sessions', ({ request: r }) => {
        url = r.url
        return HttpResponse.json({ items: [], next_cursor: null })
      }),
    )
    await endpoints.listSessions({ status: 'failed', limit: 10, cursor: 'c1' })
    expect(new URL(url).search).toBe('?status=failed&limit=10&cursor=c1')
  })
})
