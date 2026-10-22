import type { HeadroomStatus } from '@/api/types'

export type HeadroomLevel = 'ok' | 'warn' | 'critical' | 'unknown'

/** Green above 30 percent, amber 10 to 30, red below 10 or exhausted (design 3.1). */
export function headroomLevel(pct: number | null | undefined, status?: HeadroomStatus): HeadroomLevel {
  if (status === 'exhausted') return 'critical'
  if (pct == null || Number.isNaN(pct) || status === 'unknown') return 'unknown'
  if (pct < 10) return 'critical'
  if (pct <= 30) return 'warn'
  return 'ok'
}
