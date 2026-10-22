import type { SessionFilter } from '@/api/keys'
import type { SessionStatus } from '@/api/types'

export const STATUS_FILTERS: { value: SessionStatus | 'all'; label: string }[] = [
  { value: 'all', label: 'All' },
  { value: 'active', label: 'Active' },
  { value: 'escalated', label: 'Escalated' },
  { value: 'failed', label: 'Failed' },
]

const STATUSES = new Set<string>(['active', 'escalated', 'failed'])

/** Reads `?status=&q=` into a query filter. Unknown status values are ignored. */
export function filterFromParams(params: URLSearchParams): SessionFilter {
  const status = params.get('status')
  const q = params.get('q')?.trim()
  return {
    ...(status && STATUSES.has(status) ? { status: status as SessionStatus } : {}),
    ...(q ? { q } : {}),
  }
}

/** Writes a filter back, dropping empty values so the URL stays clean. */
export function paramsFromFilter(filter: SessionFilter): URLSearchParams {
  const p = new URLSearchParams()
  if (filter.status) p.set('status', filter.status)
  if (filter.q) p.set('q', filter.q)
  return p
}
