import type { ApiErrorBody } from './types'

export const API_BASE = '/api/v1'

/** JSON-RPC codes from `crates/so-core/src/proto.rs`. */
export const ERROR_CODES = {
  DEPTH_LIMIT: -32001,
  CAP_EXCEEDED: -32002,
  NOT_FOUND: -32003,
  UNAVAILABLE: -32004,
  INVALID_PARAMS: -32602,
} as const

/** Error from the API, or from the network (`status` 0, `code` 0). */
export class ApiError extends Error {
  readonly status: number
  readonly code: number
  readonly data?: unknown

  constructor(status: number, code: number, message: string, data?: unknown) {
    super(message)
    this.name = 'ApiError'
    this.status = status
    this.code = code
    this.data = data
  }

  get isUnauthorized() {
    return this.status === 401
  }
  get isNotFound() {
    return this.status === 404
  }
  get isUnavailable() {
    return this.status === 503 || this.code === ERROR_CODES.UNAVAILABLE
  }
  /** 404 or 501: the endpoint does not exist on this supervisor. */
  get isNotAvailable() {
    return this.status === 404 || this.status === 501
  }
}

type UnauthorizedHandler = () => void

function defaultUnauthorized() {
  if (typeof window !== 'undefined' && window.location.pathname !== '/token') {
    window.location.assign('/token')
  }
}

let onUnauthorized: UnauthorizedHandler = defaultUnauthorized

/** Replaces the 401 handler (the router installs one that navigates to /token). Returns a restore function. */
export function setUnauthorizedHandler(handler: UnauthorizedHandler | null): () => void {
  const prev = onUnauthorized
  onUnauthorized = handler ?? defaultUnauthorized
  return () => {
    onUnauthorized = prev
  }
}

export type Query = Record<string, string | number | boolean | null | undefined>

export interface RequestOptions {
  method?: 'GET' | 'POST' | 'PUT' | 'PATCH' | 'DELETE'
  query?: Query
  body?: unknown
  signal?: AbortSignal
}

export function buildUrl(path: string, query?: Query): string {
  const params = new URLSearchParams()
  for (const [k, v] of Object.entries(query ?? {})) {
    if (v !== undefined && v !== null && v !== '') params.set(k, String(v))
  }
  const qs = params.toString()
  return `${API_BASE}${path}${qs ? `?${qs}` : ''}`
}

/** Maps a failed response to an ApiError, tolerating non-JSON bodies. */
export async function toApiError(res: Response): Promise<ApiError> {
  let body: Partial<ApiErrorBody> | undefined
  try {
    body = (await res.json()) as Partial<ApiErrorBody>
  } catch {
    /* not JSON */
  }
  const e = body?.error
  if (e && typeof e.message === 'string') {
    return new ApiError(res.status, typeof e.code === 'number' ? e.code : 0, e.message, e.data)
  }
  return new ApiError(res.status, 0, res.statusText || `HTTP ${res.status}`)
}

export async function request<T>(path: string, opts: RequestOptions = {}): Promise<T> {
  const { method = 'GET', query, body, signal } = opts
  const headers: Record<string, string> = { Accept: 'application/json' }
  let payload: string | undefined
  if (body !== undefined) {
    headers['Content-Type'] = 'application/json'
    payload = JSON.stringify(body)
  } else if (method !== 'GET') {
    // State-changing requests always send JSON content type (design 7.5).
    headers['Content-Type'] = 'application/json'
  }
  const url = new URL(buildUrl(path, query), globalThis.location?.origin ?? 'http://localhost')
  let res: Response
  try {
    res = await fetch(url, { method, headers, body: payload, signal, credentials: 'same-origin' })
  } catch (err) {
    if (err instanceof DOMException && err.name === 'AbortError') throw err
    throw new ApiError(0, 0, 'Network error: could not reach the supervisor')
  }
  if (!res.ok) {
    const error = await toApiError(res)
    if (res.status === 401) onUnauthorized()
    throw error
  }
  if (res.status === 204) return undefined as T
  const text = await res.text()
  if (!text) return undefined as T
  try {
    return JSON.parse(text) as T
  } catch {
    throw new ApiError(res.status, 0, 'The supervisor returned an invalid response')
  }
}
